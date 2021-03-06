mod options {
    use argh::FromArgs;
    use gitoxide_core as core;
    use std::path::PathBuf;

    #[derive(FromArgs)]
    #[argh(name = "gix-plumbing")]
    /// The lean git underworld
    pub struct Args {
        #[argh(switch)]
        /// print the program version.
        pub version: bool,

        /// display verbose messages and progress information
        #[argh(switch, short = 'v')]
        pub verbose: bool,

        #[argh(option, short = 't')]
        /// the amount of threads to use for some operations.
        ///
        /// If unset, or the value is 0, there is no limit and all logical cores can be used.
        pub threads: Option<usize>,

        #[argh(subcommand)]
        pub subcommand: SubCommands,
    }

    #[derive(FromArgs, PartialEq, Debug)]
    #[argh(subcommand)]
    pub enum SubCommands {
        PackVerify(PackVerify),
        PackExplode(PackExplode),
        IndexFromPack(IndexFromPack),
    }
    /// Create an index from a packfile.
    ///
    /// This command can also be used to stream packs to standard input or to repair partial packs.
    #[derive(FromArgs, PartialEq, Debug)]
    #[argh(subcommand, name = "pack-index-from-data")]
    pub struct IndexFromPack {
        /// specify how to iterate the pack, defaults to 'verify'
        ///
        /// Valid values are
        ///
        ///  **as-is** do not do anything and expect the pack file to be valid as per the trailing hash,
        ///  **verify** the input ourselves and validate that it matches with the hash provided in the pack,
        ///  **restore** hash the input ourselves and ignore failing entries, instead finish the pack with the hash we computed
        #[argh(option, short = 'i')]
        pub iteration_mode: Option<core::pack::index::IterationMode>,

        /// path to the pack file to read (with .pack extension).
        ///
        /// If unset, the pack file is expected on stdin.
        #[argh(option, short = 'p')]
        pub pack_path: Option<PathBuf>,

        /// the folder into which to place the pack and the generated index file
        ///
        /// If unset, only informational output will be provided to standard output.
        #[argh(positional)]
        pub directory: Option<PathBuf>,
    }
    /// Explode a pack into loose objects.
    ///
    /// This can be useful in case of partially invalidated packs to extract as much information as possible,
    /// or because working with loose objects is easier with custom tooling.
    #[derive(FromArgs, PartialEq, Debug)]
    #[argh(subcommand, name = "pack-explode")]
    pub struct PackExplode {
        #[argh(switch)]
        /// read written objects back and assert they match their source. Fail the operation otherwise.
        ///
        /// Only relevant if an object directory is set.
        pub verify: bool,

        /// delete the pack and index file after the operation is successful
        #[argh(switch)]
        pub delete_pack: bool,

        /// compress bytes even when using the sink, i.e. no object directory is specified
        ///
        /// This helps to determine overhead related to compression. If unset, the sink will
        /// only create hashes from bytes, which is usually limited by the speed at which input
        /// can be obtained.
        #[argh(switch)]
        pub sink_compress: bool,

        /// the amount of checks to run. Defaults to 'all'.
        ///
        /// Allowed values:
        /// all
        /// skip-file-checksum
        /// skip-file-and-object-checksum
        /// skip-file-and-object-checksum-and-no-abort-on-decode
        #[argh(option, short = 'c')]
        pub check: Option<core::pack::explode::SafetyCheck>,

        /// the '.pack' or '.idx' file to explode into loose objects
        #[argh(positional)]
        pub pack_path: PathBuf,

        /// the path into which all objects should be written. Commonly '.git/objects'
        #[argh(positional)]
        pub object_path: Option<PathBuf>,
    }

    /// Verify a pack
    #[derive(FromArgs, PartialEq, Debug)]
    #[argh(subcommand, name = "pack-verify")]
    pub struct PackVerify {
        #[argh(switch)]
        /// decode and parse tags, commits and trees to validate their correctness beyond hashing correctly.
        ///
        /// Malformed objects should not usually occur, but could be injected on purpose or accident.
        /// This will reduce overall performance.
        pub decode: bool,

        #[argh(switch)]
        /// decode and parse tags, commits and trees to validate their correctness, and re-encode them.
        ///
        /// This flag is primarily to test the implementation of encoding, and requires to decode the object first.
        /// Encoding an object after decoding it should yield exactly the same bytes.
        /// This will reduce overall performance even more, as re-encoding requires to transform zero-copy objects into
        /// owned objects, causing plenty of allocation to occour.
        pub re_encode: bool,

        #[argh(option)]
        /// the algorithm used to verify the pack. They differ in costs.
        ///
        /// Possible values are "less-time" and "less-memory". Default is "less-memory".
        pub algorithm: Option<core::pack::verify::Algorithm>,

        /// output statistical information about the pack
        #[argh(switch, short = 's')]
        pub statistics: bool,
        /// the '.pack' or '.idx' file whose checksum to validate.
        #[argh(positional)]
        pub path: PathBuf,
    }
}

use crate::shared::ProgressRange;
use anyhow::Result;
use git_features::progress;
use gitoxide_core::{self as core, OutputFormat};
use std::io::{self, stderr, stdout};

#[cfg(not(any(feature = "prodash-render-line-crossterm", feature = "prodash-render-line-termion")))]
fn prepare(verbose: bool, name: &str, _: impl Into<Option<ProgressRange>>) -> ((), Option<prodash::progress::Log>) {
    super::init_env_logger(verbose);
    ((), Some(prodash::progress::Log::new(name, Some(1))))
}

#[cfg(any(feature = "prodash-render-line-crossterm", feature = "prodash-render-line-termion"))]
fn prepare(
    verbose: bool,
    name: &str,
    range: impl Into<Option<ProgressRange>>,
) -> (Option<prodash::render::line::JoinHandle>, Option<prodash::tree::Item>) {
    use crate::shared::{self, STANDARD_RANGE};
    super::init_env_logger(false);

    if verbose {
        let progress = prodash::Tree::new();
        let sub_progress = progress.add_child(name);
        let handle = shared::setup_line_renderer_range(progress, range.into().unwrap_or(STANDARD_RANGE), true);
        (Some(handle), Some(sub_progress))
    } else {
        (None, None)
    }
}

pub fn main() -> Result<()> {
    pub use options::*;
    let cli: Args = crate::shared::from_env();
    git_features::interrupt::init_handler(std::io::stderr());
    let thread_limit = cli.threads;
    let verbose = cli.verbose;
    match cli.subcommand {
        SubCommands::IndexFromPack(IndexFromPack {
            iteration_mode,
            pack_path,
            directory,
        }) => {
            let (_handle, progress) = prepare(verbose, "pack-explode", core::pack::index::PROGRESS_RANGE);
            core::pack::index::from_pack(
                pack_path,
                directory,
                progress::DoOrDiscard::from(progress),
                core::pack::index::Context {
                    thread_limit,
                    iteration_mode: iteration_mode.unwrap_or_default(),
                    format: OutputFormat::Human,
                    out: io::stdout(),
                },
            )
        }
        SubCommands::PackExplode(PackExplode {
            pack_path,
            sink_compress,
            object_path,
            verify,
            check,
            delete_pack,
        }) => {
            let (_handle, progress) = prepare(verbose, "pack-explode", None);
            core::pack::explode::pack_or_pack_index(
                pack_path,
                object_path,
                check.unwrap_or_default(),
                progress,
                core::pack::explode::Context {
                    thread_limit,
                    delete_pack,
                    sink_compress,
                    verify,
                },
            )
        }
        SubCommands::PackVerify(PackVerify {
            path,
            statistics,
            algorithm,
            decode,
            re_encode,
        }) => {
            use self::core::pack::verify;
            let (_handle, progress) = prepare(verbose, "pack-verify", None);
            core::pack::verify::pack_or_pack_index(
                path,
                progress,
                core::pack::verify::Context {
                    output_statistics: if statistics {
                        Some(core::OutputFormat::Human)
                    } else {
                        None
                    },
                    algorithm: algorithm.unwrap_or(verify::Algorithm::LessTime),
                    thread_limit,
                    mode: match (decode, re_encode) {
                        (true, false) => verify::Mode::Sha1CRC32Decode,
                        (true, true) | (false, true) => verify::Mode::Sha1CRC32DecodeEncode,
                        (false, false) => verify::Mode::Sha1CRC32,
                    },
                    out: stdout(),
                    err: stderr(),
                },
            )
            .map(|_| ())
        }
    }
}
