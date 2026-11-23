-- Interface language and theme per profile (D-287, localization/themes plan task 4). NULL means
-- "not chosen": the browser then falls back to the device choice, the browser language and the
-- defaults. Only enum identifiers are stored, never CSS or paths.
ALTER TABLE user_settings
    ADD COLUMN ui_locale text CHECK (ui_locale IN ('en', 'pl')),
    ADD COLUMN ui_theme  text CHECK (ui_theme IN ('midnight-slate', 'graphite', 'daylight'));
