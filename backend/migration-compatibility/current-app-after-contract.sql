INSERT INTO migration_compatibility_accounts (id, display_name)
VALUES (3, 'post-contract-user');

DO $$
BEGIN
    IF (SELECT display_name FROM migration_compatibility_accounts WHERE id = 3) <> 'post-contract-user' THEN
        RAISE EXCEPTION 'current application fixture could not write after contract';
    END IF;
END
$$;
