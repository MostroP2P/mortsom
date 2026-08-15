//! Whole-configuration validation: every section reports every problem and
//! the caller receives all of them at once.

use super::{Config, ConfigError};

impl Config {
    /// Validates every section. Returns all problems together.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let mut errors = Vec::new();
        self.mobile.validate(&mut errors);
        self.android.validate(&mut errors);
        self.relay.validate(&mut errors);
        self.mostro.validate(&mut errors);
        self.wallets.validate(&mut errors);
        self.run.validate(&mut errors);
        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigError::Invalid(errors))
        }
    }
}
