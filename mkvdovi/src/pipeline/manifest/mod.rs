//! `mkvdovi manifest`: the playback manifest of an HDR10 or HLG MKV (ROADMAP P15.3,
//! `docs/PLAYBACK_MANIFEST.md`). It measures the input with the AU table (or reuses a pair the
//! analyzer wrote for this file), settles the RPU parameters with the conversion's own phases, and
//! writes `<stem>.dvmanifest.json`. It never creates a temp directory, muxes, converts or deletes:
//! the only files it writes are the measurements, the analyzer log and the manifest, none of which
//! may be the input.

mod au_table;
mod build;
mod identity;
#[cfg(test)]
mod tests;

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use dovi84_composer::Composer;
use dovi_rpu_builder::manifest::PlaybackManifest;

use self::au_table::{au_table_path, Pair, AU_TABLE_CAPABILITY};
use self::identity::{is_matroska, is_same_file, FileIdentity};
use super::analyzer::{
    analysis_sampling, analyzer_executable, measurements_path_in, print_sidecar_advisories,
    report_reused_sidecar, resolve_auto_settings, run_hdr_analyzer_at,
};
use super::config::settle_configuration;
use super::measure::{fresh_analysis_args, Hdr10PlusMode, Measured};
use crate::cli::{Args, ManifestArgs};
use crate::external;
use crate::metadata::{self, HdrFormat, L1Sidecar};
use crate::progress;

/// Where one manifest run reads and writes.
#[derive(Debug)]
pub(super) struct Paths {
    pub(super) work_dir: PathBuf,
    pub(super) measurements: PathBuf,
    pub(super) sidecar: PathBuf,
    pub(super) au_table: PathBuf,
    /// The analyzer's temp files for the sidecar and the AU table.
    sidecar_tmp: PathBuf,
    au_table_tmp: PathBuf,
    pub(super) log: PathBuf,
    pub(super) out: PathBuf,
    pub(super) out_tmp: PathBuf,
}

impl Paths {
    pub(super) fn new(input_file: &str, work_dir: Option<&Path>, out: Option<&Path>) -> Self {
        let input = Path::new(input_file);
        let work_dir = work_dir.map_or_else(
            || match input.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
                _ => PathBuf::from("."),
            },
            Path::to_path_buf,
        );
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        let measurements = measurements_path_in(&work_dir, input_file);
        let out = out.map_or_else(
            || work_dir.join(format!("{stem}.dvmanifest.json")),
            Path::to_path_buf,
        );
        let out_tmp = with_tmp_suffix(&out);
        let sidecar = metadata::l1_sidecar_path(&measurements);
        let au_table = au_table_path(&measurements);
        Self {
            sidecar_tmp: with_tmp_suffix(&sidecar),
            au_table_tmp: with_tmp_suffix(&au_table),
            sidecar,
            au_table,
            log: work_dir.join(format!("{stem}.manifest-analyzer.log")),
            measurements,
            out,
            out_tmp,
            work_dir,
        }
    }

    /// Every path this run writes. The analyzer also writes the sidecar and the AU table, each
    /// through a `.tmp` file it creates with truncation (`hdr_analyzer_mvp` `l1_sidecar.rs`,
    /// `au_table.rs`).
    fn written(&self) -> [&Path; 8] {
        [
            &self.measurements,
            &self.sidecar,
            &self.sidecar_tmp,
            &self.au_table,
            &self.au_table_tmp,
            &self.log,
            &self.out,
            &self.out_tmp,
        ]
    }

    /// The first written path that is the input itself, under any name.
    pub(super) fn aliasing_input(&self, input: &Path) -> Option<&Path> {
        self.written()
            .into_iter()
            .find(|path| is_same_file(path, input))
    }
}

/// `path` with `.tmp` appended, the temp name of a write through a rename.
fn with_tmp_suffix(path: &Path) -> PathBuf {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    PathBuf::from(temp)
}

/// Runs `mkvdovi manifest`. `Ok(false)`: refused, with the reason printed.
pub fn manifest_file(manifest_args: &ManifestArgs) -> Result<bool> {
    progress::set_verbose(manifest_args.verbose);
    progress::set_quiet(manifest_args.quiet);
    let mut args = manifest_args.conversion_args();
    // As `main` checks them for a conversion, before any file is touched.
    match args.trim_targets_nits() {
        Ok(trims) if !trims.is_empty() => {}
        Ok(_) => anyhow::bail!("--trim-targets cannot be empty"),
        Err(_) => anyhow::bail!("--trim-targets must be a comma-separated list of integers."),
    }
    if let Some(missing) = missing_tool() {
        progress::print_error(&missing);
        return Ok(false);
    }
    resolve_auto_settings(&mut args);

    let input_file = manifest_args.input.as_str();
    let input = Path::new(input_file);
    let Some((hdr_type, hlg_composer)) = admit_input(input_file, &args)? else {
        return Ok(false);
    };
    let paths = Paths::new(
        input_file,
        manifest_args.work_dir.as_deref(),
        manifest_args.out.as_deref(),
    );
    if let Some(alias) = paths.aliasing_input(input) {
        progress::print_error(&format!(
            "'{}' is the input file; choose another --out or --work-dir.",
            alias.display()
        ));
        return Ok(false);
    }
    fs::create_dir_all(&paths.work_dir).with_context(|| {
        format!(
            "cannot create the work directory {}",
            paths.work_dir.display()
        )
    })?;

    let identity =
        FileIdentity::of(input).with_context(|| format!("cannot read {}", input.display()))?;
    let Some((mut measured, pair)) =
        acquire_pair(input_file, &args, hdr_type, hlg_composer, &paths, &identity)?
    else {
        return Ok(false);
    };
    let Some(settled) = settle_configuration(input_file, &args, hlg_composer, &mut measured, None)
    else {
        return Ok(false);
    };
    let sidecar = measured
        .l1_sidecar
        .as_ref()
        .context("the configuration settled without the measured L1 sidecar")?;
    if let Err(reason) = pair.check_sidecar(sidecar) {
        progress::print_error(&format!(
            "The measurements and the AU table do not describe the same stream: {reason}."
        ));
        return Ok(false);
    }

    let trims = args.trim_targets_nits().unwrap_or_default();
    let manifest = build::manifest(&build::Inputs {
        file_name: input
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        identity,
        pair: &pair,
        sidecar,
        settled: &settled,
        trim_targets: &trims,
        hlg_composer,
    })?;
    write_manifest(&manifest, &paths)?;
    let class = manifest
        .coverage
        .first()
        .map_or("none", |range| quality_name(range.class));
    progress::print_info(&format!(
        "Manifest written: {} ({} shots over {} pictures, coverage {class}, lookup key {}).",
        paths.out.display(),
        manifest.shots.len(),
        manifest.aus.picture_count(),
        manifest.source.lookup_key
    ));
    Ok(true)
}

fn quality_name(class: dovi_rpu_builder::manifest::QualityClass) -> &'static str {
    match class {
        dovi_rpu_builder::manifest::QualityClass::Full => "full",
        dovi_rpu_builder::manifest::QualityClass::Approximate => "approximate",
    }
}

/// Why the tools a manifest needs are not all there. MediaInfo and `dovi_tool` take part in the
/// format detection: without them a Dolby Vision or HDR10+ input could pass as plain HDR10, and a
/// manifest for it would describe the wrong stream.
fn missing_tool() -> Option<String> {
    for tool in ["mediainfo", "dovi_tool"] {
        if external::find_tool(tool).is_none() {
            return Some(format!(
                "mkvdovi manifest needs '{tool}' in PATH to tell Dolby Vision and HDR10+ inputs (which it refuses) from HDR10 and HLG."
            ));
        }
    }
    None
}

/// The checks before any analysis or file write: Matroska, a format the manifest covers, the
/// colour contracts the conversion applies, and an analyzer that writes the AU table. Returns the
/// detected format and the HLG composer (`None` for PQ), or `None` after printing the refusal.
fn admit_input(input_file: &str, args: &Args) -> Result<Option<(HdrFormat, Option<Composer>)>> {
    let input = Path::new(input_file);
    if !input.is_file() {
        progress::print_error(&format!("Input file not found: {input_file}"));
        return Ok(None);
    }
    if !is_matroska(input).with_context(|| format!("cannot read {input_file}"))? {
        progress::print_error(
            "The playback manifest covers Matroska input only (version 1); this file is not Matroska.",
        );
        return Ok(None);
    }
    let hdr_type = metadata::check_hdr_format(input_file);
    let hlg_composer = match hdr_type {
        HdrFormat::Hlg => Some(args.hlg_composer),
        HdrFormat::Hdr10Unsupported | HdrFormat::Hdr10WithMeasurements => None,
        other => {
            progress::print_error(&format!(
                "The playback manifest covers HDR10 and HLG input; this file is {}.{}",
                other.name(),
                refusal_hint(other)
            ));
            return Ok(None);
        }
    };
    if hlg_composer.is_some() {
        match metadata::check_hlg_colour_contract(input_file) {
            Ok(warnings) => warnings
                .iter()
                .for_each(|warning| progress::print_warn(warning)),
            Err(refusal) => {
                progress::print_error(&refusal);
                return Ok(None);
            }
        }
    } else if let Err(refusal) = metadata::check_pq_range_contract(input_file) {
        progress::print_error(&refusal);
        return Ok(None);
    }
    if !external::analyzer_lists_option(&analyzer_executable(), AU_TABLE_CAPABILITY) {
        progress::print_error(&format!(
            "mkvdovi manifest needs an hdr_analyzer_mvp that writes the AU table ({AU_TABLE_CAPABILITY}), and this analyzer's --help does not name it; rebuild or update hdr_analyzer_mvp."
        ));
        return Ok(None);
    }
    Ok(Some((hdr_type, hlg_composer)))
}

fn refusal_hint(format: HdrFormat) -> &'static str {
    match format {
        HdrFormat::Hdr10Plus => " The player has its own HDR10+ path.",
        HdrFormat::DolbyVisionMel | HdrFormat::DolbyVisionFel | HdrFormat::DolbyVisionP8 => {
            " A Dolby Vision file already carries its RPU."
        }
        _ => "",
    }
}

/// A measurement pair for the input: the one in the work directory when it is valid for this
/// input and these settings, else a fresh analysis with the AU table. `None`: refused, printed.
fn acquire_pair(
    input_file: &str,
    args: &Args,
    hdr_type: HdrFormat,
    hlg_composer: Option<Composer>,
    paths: &Paths,
    identity: &FileIdentity,
) -> Result<Option<(Measured, Pair)>> {
    let mut measured = Measured {
        hdr_type,
        measurements_file: Some(paths.measurements.clone()),
        hdr10plus_json: None,
        bl_source_file: PathBuf::from(input_file),
        l1_sidecar: None,
        hdr10plus_mode: Hdr10PlusMode::Metadata,
    };
    if let Some((sidecar, pair)) = reusable_pair(input_file, args, hlg_composer, paths, identity) {
        // `settle_configuration` loads a sidecar only when none is set.
        measured.l1_sidecar = Some(sidecar);
        return Ok(Some((measured, pair)));
    }

    let Some(mut extra_args) = fresh_analysis_args(args, hdr_type == HdrFormat::Hlg, hlg_composer)
    else {
        return Ok(None);
    };
    extra_args.push("--au-table".to_owned());
    progress::print_info(&format!(
        "Measuring {} with the AU table into {}...",
        input_file,
        paths.work_dir.display()
    ));
    let analyzed = run_hdr_analyzer_at(
        input_file,
        &paths.measurements,
        &paths.log,
        &extra_args,
        args,
    )?;
    if !analyzed || !paths.au_table.exists() {
        progress::print_error(&format!(
            "The analysis wrote no AU table (log: {}). Its messages above say why.",
            paths.log.display()
        ));
        return Ok(None);
    }
    let pair = match Pair::read(&paths.au_table, &paths.sidecar).and_then(|pair| {
        pair.check_source(Path::new(input_file), identity)
            .map(|()| pair)
    }) {
        Ok(pair) => pair,
        Err(reason) => {
            progress::print_error(&format!(
                "The analyzer's AU table cannot be used: {reason}."
            ));
            return Ok(None);
        }
    };
    Ok(Some((measured, pair)))
}

/// The work directory's pair, when every check passes: the sidecar is valid for this input and
/// composer and was sampled exactly as these settings sample, and the AU table is bound to it and
/// describes the input as it is now. Each failure says why and leads to a fresh analysis.
fn reusable_pair(
    input_file: &str,
    args: &Args,
    hlg_composer: Option<Composer>,
    paths: &Paths,
    identity: &FileIdentity,
) -> Option<(L1Sidecar, Pair)> {
    if !(paths.measurements.exists() && paths.sidecar.exists() && paths.au_table.exists()) {
        if paths.measurements.exists() {
            // For example a conversion's measurements: they are measured again, with the table.
            progress::print_info(&format!(
                "'{}' has no AU table; analyzing again to write one (it replaces these measurements).",
                paths.measurements.display()
            ));
        }
        return None;
    }
    let input = Path::new(input_file);
    let expect =
        metadata::SidecarExpectation::for_input(input, metadata::get_frame_count(input_file))
            .with_hlg_composer(hlg_composer);
    let reason = match metadata::load_l1_sidecar(&paths.measurements, &expect) {
        Err(error) => error.to_string(),
        Ok((sidecar, advisories)) => {
            let checked = check_sampling(&sidecar, args).and_then(|()| {
                let pair = Pair::read(&paths.au_table, &paths.sidecar)?;
                pair.check_source(input, identity)?;
                pair.check_sidecar(&sidecar)?;
                Ok(pair)
            });
            match checked {
                Ok(pair) => {
                    report_reused_sidecar(&sidecar, &paths.measurements, args);
                    print_sidecar_advisories(&advisories);
                    progress::print_info(&format!(
                        "Reusing the measurements and AU table in {}.",
                        paths.work_dir.display()
                    ));
                    return Some((sidecar, pair));
                }
                Err(reason) => reason,
            }
        }
    };
    progress::print_warn(&format!(
        "The measurements in {} cannot be reused ({reason}); re-analyzing.",
        paths.work_dir.display()
    ));
    None
}

/// The pair was sampled exactly as `--analysis-quality` samples: the manifest then describes what
/// was asked for, coarser or finer.
fn check_sampling(sidecar: &L1Sidecar, args: &Args) -> Result<(), String> {
    let wanted = analysis_sampling(args.analysis_quality);
    let used = sidecar
        .analysis
        .as_ref()
        .map(|analysis| (analysis.downscale, analysis.sample_rate));
    if used != Some(wanted) {
        return Err(match used {
            Some((downscale, sample_rate)) => format!(
                "analyzed at downscale {downscale} / sample-rate {sample_rate}, not {} / {}",
                wanted.0, wanted.1
            ),
            None => "the sidecar does not record its sampling".to_owned(),
        });
    }
    Ok(())
}

/// Serializes, parses the text back with the player's parser, and replaces `paths.out` through a
/// temp file created for this run only.
fn write_manifest(manifest: &PlaybackManifest, paths: &Paths) -> Result<()> {
    let value = manifest
        .to_json()
        .map_err(|error| anyhow::anyhow!("the manifest cannot be written: {error}"))?;
    let text = serde_json::to_string(&value)?;
    PlaybackManifest::parse(&text)
        .map_err(|error| anyhow::anyhow!("the written manifest does not parse back: {error}"))?;
    match fs::remove_file(&paths.out_tmp) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("cannot remove {}", paths.out_tmp.display()))
        }
        _ => {}
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&paths.out_tmp)
        .with_context(|| format!("cannot create {}", paths.out_tmp.display()))?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&paths.out_tmp, &paths.out)
        .with_context(|| format!("cannot write {}", paths.out.display()))?;
    Ok(())
}
