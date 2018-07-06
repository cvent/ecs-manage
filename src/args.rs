use rusoto_core::Region;

/// This tool does bulk operations against sub-components in a cluster. Use with great care.
#[derive(Debug, StructOpt)]
pub struct Args {
    /// AWS profile for authentication
    #[structopt(long = "profile")]
    pub profile: Option<String>,
    /// Sets the level of verbosity
    #[structopt(short = "v", long = "verbose", parse(from_occurrences), raw(global = "true"))]
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
    Info { cluster: String, region: Region },
    /// Services that have issues (mainly null-references)
    #[structopt(name = "audit")]
    Audit { cluster: String, region: Region },
    /// List services that are in source_cluster, but not in destination cluster (by name)
    #[structopt(name = "compare")]
    Compare {
        source_cluster: String,
        source_region: Region,
        destination_cluster: String,
        destination_region: Region,
    },
    /// Deploy healthy services in source_cluster into destination_cluster
    #[structopt(name = "sync")]
    Sync {
        source_cluster: String,
        source_region: Region,
        destination_cluster: String,
        destination_region: Region,
        /// The role to use for new services is '${destination_cluster}-${role_suffix}'
        role_suffix: Option<String>,
    },
    /// Make changes to services
    #[structopt(name = "update")]
    Update {
        cluster: String,
        region: Region,
        #[structopt(flatten)]
        modification: ServiceModification,
    },
}

#[derive(Debug, StructOpt, Clone)]
pub enum ServiceModification {
    #[structopt(name = "desired-count")]
    DesiredCount { count: i64 },
}
