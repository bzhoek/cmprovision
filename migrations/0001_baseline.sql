-- Mirrors the schema Laravel's migrations produce on SQLite, so an existing
-- installation's database is adopted unchanged and a fresh one is identical.

CREATE TABLE IF NOT EXISTS "users" ("id" integer not null primary key autoincrement, "name" varchar not null, "email" varchar not null, "email_verified_at" datetime, "password" varchar not null, "remember_token" varchar, "current_team_id" integer, "profile_photo_path" text, "created_at" datetime, "updated_at" datetime, "two_factor_secret" text, "two_factor_recovery_codes" text);
CREATE UNIQUE INDEX IF NOT EXISTS "users_email_unique" on "users" ("email");

CREATE TABLE IF NOT EXISTS "personal_access_tokens" ("id" integer not null primary key autoincrement, "tokenable_type" varchar not null, "tokenable_id" integer not null, "name" varchar not null, "token" varchar not null, "abilities" text, "last_used_at" datetime, "created_at" datetime, "updated_at" datetime);
CREATE INDEX IF NOT EXISTS "personal_access_tokens_tokenable_type_tokenable_id_index" on "personal_access_tokens" ("tokenable_type", "tokenable_id");
CREATE UNIQUE INDEX IF NOT EXISTS "personal_access_tokens_token_unique" on "personal_access_tokens" ("token");

CREATE TABLE IF NOT EXISTS "images" ("id" integer not null primary key autoincrement, "filename" varchar not null, "filename_on_server" varchar not null, "filename_extension" varchar not null, "sha256" varchar not null, "created_at" datetime, "updated_at" datetime, "uncompressed_sha256" varchar, "uncompressed_size" integer);

CREATE TABLE IF NOT EXISTS "scripts" ("id" integer not null primary key autoincrement, "name" varchar not null, "script_type" varchar check ("script_type" in ('postinstall', 'preinstall')) not null, "bg" tinyint(1) not null, "priority" integer not null default '100', "script" text not null, "created_at" datetime, "updated_at" datetime);
CREATE UNIQUE INDEX IF NOT EXISTS "scripts_name_unique" on "scripts" ("name");

CREATE TABLE IF NOT EXISTS "labels" ("id" integer not null primary key autoincrement, "name" varchar not null, "printer_type" varchar check ("printer_type" in ('ftp', 'command')) not null, "print_command" varchar, "ftp_hostname" varchar, "ftp_username" varchar, "ftp_password" varchar, "file_extension" varchar not null, "template" text not null, "created_at" datetime, "updated_at" datetime);
CREATE UNIQUE INDEX IF NOT EXISTS "labels_name_unique" on "labels" ("name");

CREATE TABLE IF NOT EXISTS "projects" ("id" integer not null primary key autoincrement, "name" varchar not null, "device" varchar check ("device" in ('cm4')) not null, "storage" varchar not null, "image_id" integer, "label_id" integer, "label_moment" varchar check ("label_moment" in ('never', 'preinstall', 'postinstall')) not null, "eeprom_firmware" varchar, "eeprom_settings" text, "created_at" datetime, "updated_at" datetime, "verify" tinyint(1) not null default '0', foreign key("image_id") references "images"("id") on delete set null on update cascade, foreign key("label_id") references "labels"("id") on delete set null on update cascade);
CREATE UNIQUE INDEX IF NOT EXISTS "projects_name_unique" on "projects" ("name");

CREATE TABLE IF NOT EXISTS "project_script" ("project_id" integer not null, "script_id" integer not null, foreign key("project_id") references "projects"("id") on delete cascade on update cascade, foreign key("script_id") references "scripts"("id") on delete cascade on update cascade);

CREATE TABLE IF NOT EXISTS "cms" ("id" integer not null primary key autoincrement, "serial" varchar not null, "mac" varchar not null, "model" varchar, "memory_in_gb" integer, "storage" integer, "cid" varchar, "csd" varchar, "firmware" varchar, "image_filename" varchar, "image_sha256" varchar, "pre_script_output" varchar, "post_script_output" varchar, "script_return_code" integer, "temp1" varchar, "temp2" varchar, "project_id" integer, "provisioning_board" varchar, "provisioning_started_at" datetime, "provisioning_complete_at" datetime, "created_at" datetime, "updated_at" datetime, foreign key("project_id") references "projects"("id") on delete set null on update cascade);
CREATE INDEX IF NOT EXISTS "cms_provisioning_started_at_index" on "cms" ("provisioning_started_at");
CREATE UNIQUE INDEX IF NOT EXISTS "cms_serial_unique" on "cms" ("serial");

CREATE TABLE IF NOT EXISTS "cmlogs" ("id" integer not null primary key autoincrement, "cm" varchar not null, "board" varchar, "ip" varchar, "loglevel" varchar check ("loglevel" in ('info', 'warning', 'error', 'debug')) not null, "msg" text not null, "created_at" datetime, "updated_at" datetime);
CREATE INDEX IF NOT EXISTS "cmlogs_cm_index" on "cmlogs" ("cm");

CREATE TABLE IF NOT EXISTS "settings" ("key" varchar not null, "value" varchar not null, primary key ("key"));

CREATE TABLE IF NOT EXISTS "hosts" ("id" integer not null primary key autoincrement, "ip" varchar not null, "mac" varchar not null, "hostname" varchar, "created_at" datetime, "updated_at" datetime);
CREATE UNIQUE INDEX IF NOT EXISTS "hosts_ip_unique" on "hosts" ("ip");
CREATE UNIQUE INDEX IF NOT EXISTS "hosts_mac_unique" on "hosts" ("mac");
