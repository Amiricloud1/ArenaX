UPDATE migration_compatibility_accounts
SET display_name = legacy_name
WHERE display_name IS NULL;
