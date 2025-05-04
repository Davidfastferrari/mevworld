use anyhow::Result;
use reth_db::Table;
use reth_db::{
    DatabaseEnv,
    mdbx::{Env},
};
use std::path::Path;

pub struct NodeDB {
    env:DatabaseEnv,
}

impl NodeDB {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let env =
            DatabaseEnv::<>::open(path, reth_db::mdbx::EnvironmentFlags::empty(), None)?;

        Ok(Self { env })
    }

    pub fn env(&self) -> &DatabaseEnv<> {
        &self.env
    }
}

// Example type your code was using
#[derive(Debug, Clone, Copy)]
pub enum InsertionType {
    Replace,
    Merge,
}
