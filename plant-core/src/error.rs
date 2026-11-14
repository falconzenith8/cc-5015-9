//! Crate-wide error type. All failure modes are explicit; the generator never
//! panics on bad parameters or exhausted budgets.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlantError {
    InvalidParam(String),
    BudgetExhausted { budget: &'static str, limit: usize },
    InvalidPlant(String),
    Json(String),
}

impl std::fmt::Display for PlantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlantError::InvalidParam(m) => write!(f, "invalid parameter: {m}"),
            PlantError::BudgetExhausted { budget, limit } => {
                write!(f, "budget exhausted: {budget} limit={limit}")
            }
            PlantError::InvalidPlant(m) => write!(f, "geometric validation failed: {m}"),
            PlantError::Json(m) => write!(f, "json error: {m}"),
        }
    }
}

impl std::error::Error for PlantError {}
