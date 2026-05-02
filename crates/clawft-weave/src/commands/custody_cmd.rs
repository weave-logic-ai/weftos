//! `weaver custody` — custody attestation commands.

use clap::{Args, Subcommand};

use crate::client::DaemonClient;
use crate::protocol::CustodyAttestResult;

#[derive(Args)]
pub struct CustodyArgs {
    #[command(subcommand)]
    pub command: CustodyCommand,
}

#[derive(Subcommand)]
pub enum CustodyCommand {
    /// Generate a signed custody attestation document.
    Attest,
}

pub async fn run(args: CustodyArgs) -> anyhow::Result<()> {
    let mut client = DaemonClient::connect()
        .await
        .ok_or_else(|| anyhow::anyhow!("no daemon running — start with 'weaver kernel start'"))?;

    match args.command {
        CustodyCommand::Attest => {
            let resp = client.simple_call("custody.attest").await?;
            if !resp.ok {
                anyhow::bail!("{}", resp.error.unwrap_or_default());
            }
            let att: CustodyAttestResult =
                serde_json::from_value(resp.result.unwrap_or_default())?;

            println!("Custody Attestation");
            println!("  Device ID:     {}", att.device_id);
            println!("  Epoch:         {}", att.epoch);
            println!("  Chain Head:    {}...", &att.chain_head[..16.min(att.chain_head.len())]);
            println!("  Chain Depth:   {}", att.chain_depth);
            println!("  Vector Count:  {}", att.vector_count);
            println!("  Content Hash:  {}...", &att.content_hash[..16.min(att.content_hash.len())]);
            println!("  Timestamp:     {}", att.timestamp);
            println!("  Signature:     {}...", &att.signature[..32.min(att.signature.len())]);

            // Also print raw JSON for machine consumption
            println!();
            println!("JSON:");
            let json = serde_json::to_string_pretty(&att)?;
            println!("{json}");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn custody_args_parses() {
        // Verify clap derivation is well-formed.
        #[derive(clap::Parser)]
        struct Wrapper {
            #[command(subcommand)]
            cmd: WrapperCmd,
        }
        #[derive(clap::Subcommand)]
        enum WrapperCmd {
            Custody(CustodyArgs),
        }
        Wrapper::command().debug_assert();
    }
}
