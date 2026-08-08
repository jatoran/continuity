//! Packed npm consumer contract runners.

use std::path::Path;
use std::process::Command;

use anyhow::Result;

use super::{executable_name, require_success};

pub(super) fn run_node_contracts(consumer: &Path) -> Result<()> {
    run_contract(
        consumer,
        "controlled-controller.test.mjs",
        "packed controlled-controller contract",
    )?;
    run_contract(
        consumer,
        "coordinates.test.mjs",
        "packed coordinate contract",
    )?;
    run_contract(
        consumer,
        "source_lines.test.mjs",
        "packed source-line mirror contract",
    )?;
    run_contract(
        consumer,
        "embedding-contracts.test.mjs",
        "packed embedding helper and SSR contract",
    )?;
    run_contract(
        consumer,
        "composition-defer.test.mjs",
        "packed IME composition deferral contract",
    )?;
    run_contract(
        consumer,
        "selection-actions-placement.test.mjs",
        "packed touch selection action bar placement contract",
    )
}

fn run_contract(consumer: &Path, script: &str, label: &str) -> Result<()> {
    let mut test = Command::new(executable_name("node"));
    test.current_dir(consumer).arg(script);
    require_success(&mut test, label)
}
