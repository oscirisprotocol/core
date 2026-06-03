use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChainConfig {
    pub rpc_url: String,
    pub provider_registry: Option<String>,
    pub job_escrow: Option<String>,
    pub receipt_registry: Option<String>,
}

impl ChainConfig {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.rpc_url.trim().is_empty() {
            return Err("rpc_url must not be empty");
        }
        Ok(())
    }
}
