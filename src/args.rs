use failure::Error;
use rusoto_core::Region;
use serde_json;
use std::str::FromStr;

use std::collections::HashMap;
use std::fs::File;

/// This tool does bulk operations against sub-components in a cluster. Use with great care.
#[derive(Debug, StructOpt)]
pub struct Args {
    /// AWS profile for authentication
    #[structopt(long = "profile")]
    pub profile: Option<String>,
    /// Sets the level of verbosity
    #[structopt(
        short = "v",
        long = "verbose",
        parse(from_occurrences),
        raw(global = "true")
    )]
    pub verbosity: usize,
    /// Sub commands
    #[structopt(subcommand)]
    pub command: EcsCommand,
}

#[derive(Debug, StructOpt)]
pub enum EcsCommand {
    /// Do operations on all services within a cluster
    #[structopt(name = "services")]
    ServicesCommand {
        /// Sub commands
        #[structopt(subcommand)]
        command: ServicesCommand,
    },
}

#[derive(Debug, StructOpt)]
pub enum ServicesCommand {
    /// Useful information about services
    #[structopt(name = "info")]
    Info {
        /// The cluster name
        cluster: String,
        /// The AWS region
        region: Region,
    },
    /// Services that have issues (mainly null-references)
    #[structopt(name = "audit")]
    Audit {
        /// The cluster name
        cluster: String,
        /// The AWS region
        region: Region,
    },
    /// List services that are in source_cluster, but not in destination cluster (by name)
    #[structopt(name = "compare")]
    Compare {
        /// The source cluster name
        source_cluster: String,
        /// The source AWS region
        source_region: Region,
        /// The destination cluster name
        destination_cluster: String,
        /// The destination AWS region
        destination_region: Region,
    },
    /// Deploy healthy services in source_cluster into destination_cluster
    #[structopt(name = "sync")]
    Sync {
        /// The source cluster name
        source_cluster: String,
        /// The source AWS region
        source_region: Region,
        /// The destination cluster name
        destination_cluster: String,
        /// The destination AWS region
        destination_region: Region,
        /// The role to use for new services is '${destination_cluster}-${role_suffix}'
        role_suffix: Option<String>,
    },
    /// Export properties of services in a format that `update` understands
    #[structopt(name = "export")]
    Export {
        /// The cluster name
        cluster: String,
        /// The AWS region
        region: Region,
        #[structopt(
            raw(
                possible_values = "&ServiceProperty::variants()",
                case_insensitive = "true"
            )
        )]
        property: ServiceProperty,
    },
    /// Make changes to services
    #[structopt(name = "update")]
    Update {
        /// The cluster name
        cluster: String,
        /// The AWS region
        region: Region,
        #[structopt(flatten)]
        modification: ServiceModification,
    },
}

arg_enum!{
    #[derive(Debug)]
    pub enum ServiceProperty {
        DesiredCount,
    }
}

#[derive(Debug, StructOpt, Clone)]
pub enum ServiceModification {
    #[structopt(name = "desired-count")]
    DesiredCount {
        /// Either an integer to set desired count for all services,
        /// or a path to a JSON file containing a service name -> desired count mapping
        count: DesiredCountOptions,
    },
}

#[derive(Debug, Clone)]
pub enum DesiredCountOptions {
    Number(i64),
    Map(HashMap<String, i64>),
}

impl FromStr for DesiredCountOptions {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Ok(count) = s.parse::<i64>() {
            Ok(DesiredCountOptions::Number(count))
        } else {
            Ok(DesiredCountOptions::Map(serde_json::from_reader(
                File::open(s)?,
            )?))
        }
    }
}
