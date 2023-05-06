use clap::Parser;
use json::JsonValue;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::usize;
use thiserror::Error;

#[derive(Parser, Debug)]
struct Args {
    #[arg(short, long)]
    top: String,

    #[arg(short, long)]
    array: String,

    #[arg(short, long)]
    output: String,
}

// TODO: Use anyhow (sic!) crate instead?
#[derive(Error, Debug)]
enum RechunkingError {
    #[error("TODO")]
    IncompatibleZarrVersion,

    #[error("TODO")]
    IncompatibleChunkSize,

    #[error("TODO")]
    IncompatibleArrayShape,

    #[error("TODO")]
    InvalidJSON,

    #[error("TODO")]
    InvalidMetadataFile,

    #[error("TODO")]
    InvalidChunkFiles,

    #[error("TODO")]
    InvalidArgError,

    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

struct Metadata {
    // TODO: Add compression options
    json: JsonValue,
    shape: usize,
}

fn parse_zarray(in_dir: &Path) -> Result<Metadata, RechunkingError> {
    let zarray_file = in_dir.join(".zarray");
    let json = json::parse(&fs::read_to_string(zarray_file.as_path())?)
        .map_err(|_| RechunkingError::InvalidJSON)?;
    if !json.has_key("zarr_format") || json["zarr_format"] != 2 {
        return Err(RechunkingError::IncompatibleZarrVersion);
    }
    if !json.has_key("chunks") || json["chunks"].members().as_slice() != [1] {
        return Err(RechunkingError::IncompatibleChunkSize);
    }
    if !json.has_key("shape") {
        return Err(RechunkingError::IncompatibleArrayShape);
    }
    let [s] = json["shape"].members().as_slice() else {
        return Err(RechunkingError::IncompatibleArrayShape)
    };
    let shape = s
        .as_usize()
        .ok_or(RechunkingError::IncompatibleArrayShape)?;
    Ok(Metadata { json, shape })
}

fn collect_chunks(chunks_dir: &Path, shape: usize) -> Result<Vec<PathBuf>, RechunkingError> {
    // TODO: Return Path or PathBuf?
    // TODO: Should unexpected files in dir result in error too?
    let mut chunks = fs::read_dir(chunks_dir)?
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let idx = path
                .file_name()?
                .to_str()?
                .to_string()
                .parse::<usize>()
                .ok()?;
            if !path.is_file() {
                return None;
            }
            Some((idx, path))
        })
        .collect::<Vec<(usize, PathBuf)>>();

    let num_chunks = chunks.len();
    if num_chunks > shape {
        // TODO: error msg: found too many chunks for given shape
        return Err(RechunkingError::InvalidChunkFiles);
    }

    chunks.sort_by_key(|p| p.0);

    let (idxs, paths): (Vec<usize>, Vec<PathBuf>) = chunks.into_iter().unzip();

    if !idxs.into_iter().eq(0..num_chunks) {
        // TODO: error msg: chunks indices are not consecutive
        return Err(RechunkingError::InvalidChunkFiles);
    }

    Ok(paths)
}

fn concat_chunks(paths: Vec<PathBuf>) -> Vec<u8> {
    // TODO: Implement parallel processing
    // TODO: Error handling in this function?
    paths
        .iter()
        .flat_map(|p| fs::read(p.as_path()))
        .flat_map(|b| unsafe { blosc::decompress_bytes::<u8>(&b[..]) }.unwrap_or(vec![]))
        .collect()
}

fn write_chunk(out_path: &Path, arr_buf: Vec<u8>) -> io::Result<()> {
    // TODO: Handle errors (out_path exist, cannot be created)
    fs::create_dir(out_path)?;
    // TODO: Implement compression.
    fs::write(out_path.join("0"), arr_buf)
}

fn adjust_zarray(in_data: JsonValue, chunk_size: usize) -> JsonValue {
    let mut out = in_data;
    out["chunks"] = json::array![chunk_size];
    // TODO: Adapt compression options
    out
}

fn write_metadata(
    top_dir: &Path,
    rel_in_dir: &Path,
    rel_out_dir: &Path,
    out_data: JsonValue,
) -> Result<(), RechunkingError> {
    let out_zarray = rel_out_dir.join(".zarray");
    let out_zattrs = rel_out_dir.join(".zattrs");
    let in_zattrs = rel_in_dir.join(".zattrs");
    let in_out_zmetadata = top_dir.join(".zmetadata");

    if in_out_zmetadata.exists() {
        let mut top_json = json::parse(&fs::read_to_string(&in_out_zmetadata)?)
            .map_err(|_| RechunkingError::InvalidJSON)?;
        if !top_json.has_key("metadata") {
            return Err(RechunkingError::InvalidMetadataFile);
        }

        let metadata = &mut top_json["metadata"];
        let out_zarray_str: &str = &out_zarray.to_string_lossy();
        // NOTE: No check if metadata contains zarray key of input
        metadata[out_zarray_str] = out_data.clone();

        let in_zattrs_str: &str = &in_zattrs.to_string_lossy();
        if metadata.has_key(in_zattrs_str) {
            let out_zattrs_str: &str = &out_zattrs.to_string_lossy();
            metadata[out_zattrs_str] = metadata[in_zattrs_str].clone();
        }

        // TODO: Why does fs::write not always overwrite as documented?
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(in_out_zmetadata)?;
        f.write_all(json::stringify_pretty(top_json, 4).as_bytes())?;
    }

    if top_dir.join(&in_zattrs).exists() {
        fs::copy(top_dir.join(in_zattrs), top_dir.join(out_zattrs))?;
    }

    fs::write(
        top_dir.join(out_zarray),
        json::stringify_pretty(out_data, 4),
    )?;
    Ok(())
}

fn is_normal_comp(path: &Path) -> bool {
    let comps: Vec<Component> = path.components().collect();
    comps.len() == 1 && comps.iter().all(|c| matches!(c, Component::Normal { .. }))
}

fn main() -> Result<(), RechunkingError> {
    let args = Args::parse();
    // TODO: Move args checks into separate fn?
    let top_dir = Path::new(&args.top);
    if !top_dir.is_dir() {
        // TODO: error msg.
        return Err(RechunkingError::InvalidArgError);
    }
    let rel_in_dir = Path::new(&args.array);
    let in_dir = top_dir.join(rel_in_dir);
    if !in_dir.is_dir() {
        // TODO: error msg.
        return Err(RechunkingError::InvalidArgError);
    }
    let out_comp = Path::new(&args.output);
    if !is_normal_comp(out_comp) {
        // TODO: error msg.
        return Err(RechunkingError::InvalidArgError);
    }
    let rel_out_dir = rel_in_dir
        .parent()
        .ok_or(RechunkingError::InvalidArgError)?
        .join(out_comp);
    let out_dir = top_dir.join(rel_out_dir.clone());
    // TODO: Handle case that out_dir exits or is equal to array_dir (forbid overwriting?!)
    let metadata = parse_zarray(&in_dir)?;
    let chunks = collect_chunks(&in_dir, metadata.shape)?;
    let num_chunks = chunks.len();
    write_chunk(&out_dir, concat_chunks(chunks))?;
    write_metadata(
        top_dir,
        rel_in_dir,
        &rel_out_dir,
        adjust_zarray(metadata.json, num_chunks),
    )?;
    Ok(())
}
