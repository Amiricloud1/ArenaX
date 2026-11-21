DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM migration_compatibility_accounts
        WHERE display_name IS NULL
    ) THEN
        RAISE EXCEPTION 'current application fixture found an unbackfilled display_name';
    END IF;
END
$$;

INSERT INTO migration_compatibility_accounts (id, legacy_name, display_name)
VALUES (2, 'new-user', 'new-user')
ON CONFLICT (id) DO UPDATE SET display_name = EXCLUDED.display_name;
