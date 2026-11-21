use std::collections::HashSet;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use arenax_backend::service::payment_provider::{
    DepositInitiation, PaymentError, PaymentProvider, PaymentStatus,
};

#[derive(Clone)]
pub struct TestClock(Arc<AtomicI64>);

impl TestClock {
    pub fn new(now: i64) -> Self {
        Self(Arc::new(AtomicI64::new(now)))
    }

    pub fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }

    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy)]
pub enum Response {
    Accepted,
    Failed,
    TransportError,
}

struct ScheduledResponse {
    reference: String,
    ready_at: i64,
    sequence: u64,
    outcome: Response,
}

pub struct ProviderSimulator {
    clock: TestClock,
    responses: Mutex<Vec<ScheduledResponse>>,
    delivered: Mutex<HashSet<String>>,
}

impl ProviderSimulator {
    pub fn new(clock: TestClock) -> Self {
        Self {
            clock,
            responses: Mutex::new(Vec::new()),
            delivered: Mutex::new(HashSet::new()),
        }
    }

    pub fn schedule(
        &self,
        reference: impl Into<String>,
        ready_at: i64,
        sequence: u64,
        outcome: Response,
    ) {
        self.responses.lock().expect("responses mutex poisoned").push(
            ScheduledResponse {
                reference: reference.into(),
                ready_at,
                sequence,
                outcome,
            },
        );
    }

    fn response(&self, reference: &str) -> Result<Option<Response>, PaymentError> {
        if self
            .delivered
            .lock()
            .expect("delivered mutex poisoned")
            .contains(reference)
        {
            return Err(PaymentError::Provider(format!(
                "duplicate response for '{reference}'"
            )));
        }

        let mut responses = self.responses.lock().expect("responses mutex poisoned");
        let next = responses
            .iter()
            .enumerate()
            .filter(|(_, response)| {
                response.reference == reference && response.ready_at <= self.clock.now()
            })
            .min_by_key(|(_, response)| (response.ready_at, response.sequence))
            .map(|(index, _)| index);

        let Some(index) = next else {
            return Ok(None);
        };
        let response = responses.remove(index);
        self.delivered
            .lock()
            .expect("delivered mutex poisoned")
            .insert(reference.to_string());

        match response.outcome {
            Response::TransportError => Err(PaymentError::Provider(
                "simulated provider transport failure".into(),
            )),
            outcome => Ok(Some(outcome)),
        }
    }

    fn status(&self, reference: &str) -> Result<PaymentStatus, PaymentError> {
        Ok(match self.response(reference)? {
            None => PaymentStatus::Pending,
            Some(Response::Accepted) => PaymentStatus::Success,
            Some(Response::Failed) => PaymentStatus::Failed,
            Some(Response::TransportError) => unreachable!("transport errors return above"),
        })
    }
}

#[async_trait]
impl PaymentProvider for ProviderSimulator {
    fn name(&self) -> &'static str {
        "provider-simulator"
    }

    async fn initiate_deposit(
        &self,
        reference: &str,
        _amount: i64,
        _currency: &str,
    ) -> Result<DepositInitiation, PaymentError> {
        Ok(DepositInitiation {
            reference: reference.to_string(),
            authorization_url: Some("https://simulator.invalid/checkout".into()),
        })
    }

    async fn verify_deposit(
        &self,
        reference: &str,
        _expected_amount: i64,
    ) -> Result<bool, PaymentError> {
        Ok(self.status(reference)? == PaymentStatus::Success)
    }

    async fn initiate_withdrawal(
        &self,
        reference: &str,
        _amount: i64,
        _currency: &str,
        _destination: &str,
    ) -> Result<PaymentStatus, PaymentError> {
        self.status(reference)
    }

    async fn check_status(&self, reference: &str) -> Result<PaymentStatus, PaymentError> {
        self.status(reference)
    }
}
