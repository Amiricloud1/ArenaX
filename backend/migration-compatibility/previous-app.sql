INSERT INTO migration_compatibility_accounts (id, legacy_name)
VALUES (1, 'legacy-user')
ON CONFLICT (id) DO UPDATE SET legacy_name = EXCLUDED.legacy_name;

DO $$
BEGIN
    IF (SELECT legacy_name FROM migration_compatibility_accounts WHERE id = 1) <> 'legacy-user' THEN
        RAISE EXCEPTION 'previous application fixture could not read legacy_name';
    END IF;
END
$$;
