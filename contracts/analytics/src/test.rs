#![cfg(test)]

use super::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    BytesN, Env,
};

fn setup(env: &Env) -> (AnalyticsContractClient<'_>, Address) {
    env.mock_all_auths();
    let contract_id = env.register(AnalyticsContract, ());
    let client = AnalyticsContractClient::new(env, &contract_id);
    let admin = Address::generate(env);
    let salt = BytesN::from_array(env, &[7u8; 32]);
    client.initialize(&admin, &salt);
    (client, admin)
}

#[test]
fn test_register_and_unregister_contract() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");

    assert!(!client.is_registered_contract(&id));
    client.register_contract(&id, &reporter);
    assert!(client.is_registered_contract(&id));
    client.unregister_contract(&id);
    assert!(!client.is_registered_contract(&id));
}

#[test]
#[should_panic(expected = "contract not registered")]
fn test_record_event_rejects_unregistered() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    client.record_event(
        &Symbol::new(&env, "unknown"),
        &Symbol::new(&env, "volume"),
        &100,
    );
}

#[test]
fn test_record_event_aggregates_counters() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    client.record_event(&id, &Symbol::new(&env, "tx"), &1);
    client.record_event(&id, &Symbol::new(&env, "volume"), &500);
    client.record_event(&id, &Symbol::new(&env, "users"), &3);
    client.record_event(&id, &Symbol::new(&env, "fees"), &50);
    client.record_event(&id, &Symbol::new(&env, "transaction"), &1);
    client.record_event(&id, &Symbol::new(&env, "user"), &0);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 6);
    assert_eq!(snap.total_volume, 500);
    assert_eq!(snap.total_users, 4);
    assert_eq!(snap.total_fees_collected, 50);
}

#[test]
fn test_get_platform_snapshot_single_call() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "stake");
    client.register_contract(&id, &reporter);

    client.record_event(&id, &Symbol::new(&env, "volume"), &1000);
    client.record_event(&id, &Symbol::new(&env, "fee"), &25);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 2);
    assert_eq!(snap.total_volume, 1000);
    assert_eq!(snap.total_users, 0);
    assert_eq!(snap.total_fees_collected, 25);
}

#[test]
fn test_duplicate_event_delivery_accepted_once() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    let event_id = BytesN::from_array(&env, &[1u8; 32]);
    client.record_event(&id, &Symbol::new(&env, "volume"), &100, &event_id, &1);
    // Duplicate delivery of the same event identity must not double-count.
    client.record_event(&id, &Symbol::new(&env, "volume"), &100, &event_id, &1);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 1);
    assert_eq!(snap.total_volume, 100);
}

#[test]
fn test_out_of_order_batch_converges() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    let e1 = BytesN::from_array(&env, &[1u8; 32]);
    let e2 = BytesN::from_array(&env, &[2u8; 32]);
    let e3 = BytesN::from_array(&env, &[3u8; 32]);

    // Deliver out of order, then replay in a different order.
    client.record_event(&id, &Symbol::new(&env, "volume"), &30, &e3, &1);
    client.record_event(&id, &Symbol::new(&env, "volume"), &10, &e1, &1);
    client.record_event(&id, &Symbol::new(&env, "volume"), &20, &e2, &1);
    client.record_event(&id, &Symbol::new(&env, "volume"), &10, &e1, &1);
    client.record_event(&id, &Symbol::new(&env, "volume"), &30, &e3, &1);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 3);
    assert_eq!(snap.total_volume, 60);
}

#[test]
fn test_producer_restart_replay_accepted_once() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    let event_id = BytesN::from_array(&env, &[9u8; 32]);
    client.record_event(&id, &Symbol::new(&env, "users"), &2, &event_id, &1);
    // Producer restarts and replays the same buffered event.
    client.record_event(&id, &Symbol::new(&env, "users"), &2, &event_id, &1);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 1);
    assert_eq!(snap.total_users, 2);
}

#[test]
fn test_schema_version_rollout_dedupes_across_versions() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    let event_id = BytesN::from_array(&env, &[5u8; 32]);
    client.record_event(&id, &Symbol::new(&env, "volume"), &40, &event_id, &1);
    // Same identity re-delivered under a newer schema version is still a duplicate.
    client.record_event(&id, &Symbol::new(&env, "volume"), &40, &event_id, &2);

    let snap = client.get_platform_snapshot();
    assert_eq!(snap.total_transactions, 1);
    assert_eq!(snap.total_volume, 40);
}

#[test]
fn test_hourly_circular_overwrite_168_slots() {
    let env = Env::default();
    let (client, _admin) = setup(&env);
    let reporter = Address::generate(&env);
    let id = Symbol::new(&env, "match");
    client.register_contract(&id, &reporter);

    env.ledger().with_mut(|l| l.timestamp = 0);
    client.record_event(&id, &Symbol::new(&env, "volume"), &100);
    client.record_event(&id, &Symbol::new(&env, "volume"), &50);
    let slot0 = client.get_hourly_snapshot(&0).unwrap();
    assert_eq!(slot0.hour, 0);
    assert_eq!(slot0.total_volume, 150);
    assert_eq!(slot0.total_transactions, 2);

    env.ledger().with_mut(|l| l.timestamp = 3600);
    client.record_event(&id, &Symbol::new(&env, "volume"), &10);
    let slot1 = client.get_hourly_snapshot(&1).unwrap();
    assert_eq!(slot1.hour, 1);
    assert_eq!(slot1.total_volume, 10);

    env.ledger().with_mut(|l| l.timestamp = 168 * 3600);
    client.record_event(&id, &Symbol::new(&env, "volume"), &5);
    let wrapped = client.get_hourly_snapshot(&0).unwrap();
    assert_eq!(wrapped.hour, 168);
    assert_eq!(wrapped.total_volume, 5);
    assert_eq!(wrapped.total_transactions, 1);
}
