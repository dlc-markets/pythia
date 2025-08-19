use displaydoc::Display;
use thiserror::Error;
use tokio::task::JoinError;

use crate::{
    api::error::PythiaApiError, config::error::PythiaConfigError, oracle::error::OracleError,
    schedule_context::error::PythiaContextError,
};

#[derive(Display, Error)]
pub enum PythiaError {
    /// Error from Pythia API: {0}
    Api(#[from] PythiaApiError),

    /// Configuration Error: {0}
    Config(#[from] PythiaConfigError),

    /// Oracle failed: {0}
    Oracle(#[from] OracleError),

    /// Postgres Error: {0}
    Postgres(#[from] sqlx::Error),

    /// Context Error: {0}
    ContextSetup(#[from] PythiaContextError),

    /// Pythia API or Scheduler panicked: {0}
    Panic(#[from] JoinError),
}

// Error in main are shown using Debug display which is sad
// because we implemented a nice display of all error produced by Pythia
// So we use Display implementation as the Debug one

impl std::fmt::Debug for PythiaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}
