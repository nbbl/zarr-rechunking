use anyhow::Result;
use blosc::{Clevel, Compressor, Context, ShuffleMode};
use clap::Parser;
use json::JsonValue::Null;
use json::{object, JsonValue};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::fmt::Debug;
use std::fs::{self, OpenOptions};
use std::io::{Error, Write};
use std::path::{Component, Path, PathBuf};
use std::usize;
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The top level directory under which <ARRAY> must be placed
    #[arg(short, long)]
    top: String,

    /// Directory with input array (chunk files, .zarray, optionally .zattrs),
    /// the provided path will be interpreted as subpath of <TOP>
    #[arg(short, long, verbatim_doc_comment)]
    array: String,

    /// Single component output directory for rechunked array,
    /// the provided path must be a single component.
    /// Output will be created under parent of <TOP>/<ARRAY>
    #[arg(short, long, verbatim_doc_comment)]
    output: String,

    /// Blosc internal compression algorithm for output chunk
    #[arg(short, long)]
    #[clap(value_enum, default_value = "none")]
    compression: CompressionArg,
}

#[derive(clap::ValueEnum, Debug, PartialEq, Clone)]
#[clap(rename_all = "lower")]
enum CompressionArg {
    BloscLZ,
    LZ4,
    LZ4HC,
    Snappy,
    Zlib,
    Zstd,
    None,
}

#[derive(Error, Debug)]
enum RechunkingError {
    #[error("{0} is not a valid JSON file")]
    InvalidJSON(String),

    #[error("Zarr metadata file {0} is invalid: {1}")]
    InvalidMetadataFile(String, &'static str),

    #[error("Invalid chunk files in folder {0}: {1}")]
    InvalidChunkFiles(String, &'static str),

    #[error("Invalid command line argument passed: {0}")]
    InvalidArgError(&'static str),

    #[error("Decompression failed")]
    DecompressionError,

    #[error("Read or write failed: {0}")]
    IoError(String),
}

/// Metadata parsed from .zarray file
struct Metadata {
    // .zarray parsed to JSON
    json: JsonValue,
    // Shape of array
    shape: usize,
    // Whether chunks are compressed
    is_compressed: bool,
}

/// JSON object representing the used compression options in the result .zarray
///
/// corresponds to COMPRESSION_CONTEXT
static COMPRESSION_OPTIONS: Lazy<JsonValue> = Lazy::new(|| {
    object! {
        // Automatic determination of block size
        blocksize: 0,
        // Compression level
        clevel: 5,
        // Name of primary compressor (wrapper)
        id: "blosc",
        // Shuffle (filtering) mode, is byte shuffle
        shuffle: 1
    }
});

/// Parse the metadata file at `in_dir`/.zarray
///
/// An error is returned if the file cannot be read,
/// JSON is not valid,
/// or metadata is incompatible with rechunking
/// Note that compressor options are not checked in detail.
/// It is only checked that a Blosc compressor was used!
fn parse_zarray(in_dir: &Path) -> Result<Metadata> {
    let zarray_file = in_dir.join(".zarray");
    let zarray_file_str = zarray_file.to_string_lossy().to_string();
    let err_map = |_: Error| {
        RechunkingError::IoError(format!("File at {} could not be read", zarray_file_str))
    };
    let file_str = fs::read_to_string(zarray_file.as_path()).map_err(err_map)?;
    let json = json::parse(&file_str)
        .map_err(|_| RechunkingError::InvalidJSON(zarray_file_str.clone()))?;

    if !json.has_key("zarr_format") || json["zarr_format"] != 2 {
        return Err(RechunkingError::InvalidMetadataFile(
            zarray_file_str,
            "missing, incompabtible or invalid zarr_format value",
        )
        .into());
    }

    if !json.has_key("chunks") || json["chunks"].members().as_slice() != [1] {
        return Err(RechunkingError::InvalidMetadataFile(
            zarray_file_str,
            "missing, incompatible or invalid chunks value, must be [1]",
        )
        .into());
    }

    if !json.has_key("shape") {
        return Err(
            RechunkingError::InvalidMetadataFile(zarray_file_str, "missing shape value").into(),
        );
    }
    let [s] =  json["shape"].members().as_slice() else {
        return Err(RechunkingError::InvalidMetadataFile(
            zarray_file_str,
            "incompatible or invalid shape value, must be one-dimensional",
        ).into());
    };
    let shape = s.as_usize().ok_or(RechunkingError::InvalidMetadataFile(
        zarray_file_str.clone(),
        "shape value is not a valid dimension",
    ))?;

    if !json.has_key("compressor") {
        return Err(RechunkingError::InvalidMetadataFile(
            zarray_file_str,
            "missing compressor value",
        )
        .into());
    }
    let compressor = &json["compressor"];
    let is_compressed = if compressor.is_null() {
        false
    } else {
        if !compressor.has_key("id") {
            return Err(RechunkingError::InvalidMetadataFile(
                zarray_file_str,
                "compressor id is not specified",
            )
            .into());
        }
        if compressor["id"] != "blosc" {
            return Err(RechunkingError::InvalidMetadataFile(
                zarray_file_str,
                "only Blosc compressor is supported",
            )
            .into());
        }
        true
    };

    if !json.has_key("filters") || json["filter"] != Null {
        return Err(RechunkingError::InvalidMetadataFile(
            zarray_file_str,
            "missing, incompatible or invalid filter value.\n
            Filter must be null, filtering is only supported as part of compressor",
        )
        .into());
    }

    Ok(Metadata {
        json,
        shape,
        is_compressed,
    })
}

/// Adjust the JSON for the output .zarray
fn adjust_zarray(in_data: JsonValue, chunk_size: usize, compression: &CompressionArg) -> JsonValue {
    let mut out = in_data;
    out["chunks"] = json::array![chunk_size];

    let compressor = match &compression {
        CompressionArg::None => json::Null,
        _ => {
            let mut c = COMPRESSION_OPTIONS.clone();
            c["cname"] = json::from(format!("{:?}", &compression).to_lowercase());
            c
        }
    };
    out["compressor"] = compressor;
    out
}

/// Adjust and rewrite .zmetadata if it exists,
/// copy .zattrs from input to output dir if it exists,
/// and write .zarray to output dir
fn write_metadata(
    top_dir: &Path,
    rel_in_dir: &Path,
    rel_out_dir: &Path,
    out_data: JsonValue,
) -> Result<()> {
    let out_zarray = rel_out_dir.join(".zarray");
    let out_zattrs = rel_out_dir.join(".zattrs");
    let in_zattrs = rel_in_dir.join(".zattrs");
    let in_out_zmetadata = top_dir.join(".zmetadata");

    if in_out_zmetadata.exists() {
        let zmetadata_str = in_out_zmetadata.to_string_lossy().to_string();
        let mut top_json = json::parse(&fs::read_to_string(&in_out_zmetadata)?)
            .map_err(|_| RechunkingError::InvalidJSON(zmetadata_str.clone()))?;
        if !top_json.has_key("metadata") {
            return Err(RechunkingError::InvalidMetadataFile(
                zmetadata_str,
                "metadata value is missing",
            )
            .into());
        }

        let metadata = &mut top_json["metadata"];
        let out_zarray_str: &str = &out_zarray.to_string_lossy();
        metadata[out_zarray_str] = out_data.clone();

        let in_zattrs_str: &str = &in_zattrs.to_string_lossy();
        if metadata.has_key(in_zattrs_str) {
            let out_zattrs_str: &str = &out_zattrs.to_string_lossy();
            metadata[out_zattrs_str] = metadata[in_zattrs_str].clone();
        }

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

/// Collect the paths of chunk files in `chunk_dir`
///
/// An error is returned if the paths
/// do not form a sequence of mergeable indices,
/// or if there are too many chunk files for
/// an array of length `shape`.
/// Otherwise the paths are returned in the order
/// that their respective chunks should be merged in.
fn collect_chunk_paths(chunks_dir: &Path, shape: usize) -> Result<Vec<PathBuf>> {
    let err_map = |_: Error| {
        RechunkingError::IoError(format!(
            "Directory at {} could not be read",
            chunks_dir.to_string_lossy()
        ))
    };
    let mut chunks = fs::read_dir(chunks_dir)
        .map_err(err_map)?
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
    let chunks_dir_str = chunks_dir.to_string_lossy().to_string();
    if num_chunks == 0 {
        return Err(
            RechunkingError::InvalidChunkFiles(chunks_dir_str, "no chunk files found").into(),
        );
    }

    if num_chunks > shape {
        return Err(RechunkingError::InvalidChunkFiles(
            chunks_dir_str,
            "found too many chunks files for given shape",
        )
        .into());
    }

    chunks.sort_by_key(|p| p.0);

    let (idxs, paths): (Vec<usize>, Vec<PathBuf>) = chunks.into_iter().unzip();

    if !idxs.into_iter().eq(0..num_chunks) {
        return Err(RechunkingError::InvalidChunkFiles(
            chunks_dir_str,
            "chunk files do not form consecutive sequence 0..num_chunks",
        )
        .into());
    }

    Ok(paths)
}

/// Concatenate the chunks at the given `paths`
///
/// Decompress if `is_compressed` is true
fn concat_chunks(paths: Vec<PathBuf>, is_compressed: bool) -> Result<Vec<u8>> {
    let decompressor = if is_compressed {
        |b: Vec<u8>| Ok::<Vec<u8>, ()>(unsafe { blosc::decompress_bytes::<u8>(&b[..])? })
    } else {
        Ok
    };

    let buffers = paths
        .par_iter()
        .flat_map(|p| fs::read(p.as_path()))
        .map(decompressor)
        .collect::<Result<Vec<Vec<u8>>, ()>>()
        .map_err(|_| RechunkingError::DecompressionError)?;

    Ok(buffers.into_par_iter().flatten().collect())
}

/// Creates a compression context based on `arg`
fn make_compression_ctx(arg: &CompressionArg) -> Option<Context> {
    match arg {
        CompressionArg::BloscLZ => Some(Compressor::BloscLZ),
        CompressionArg::LZ4 => Some(Compressor::LZ4),
        CompressionArg::LZ4HC => Some(Compressor::LZ4HC),
        CompressionArg::Snappy => Some(Compressor::Snappy),
        CompressionArg::Zlib => Some(Compressor::Zlib),
        CompressionArg::Zstd => Some(Compressor::Zstd),
        CompressionArg::None => None,
    }
    .map(|compressor| {
        Context::new()
            // Automatic determination of block size
            .blocksize(None)
            // Compression algorithm
            .compressor(compressor)
            .unwrap()
            // Compression level
            .clevel(Clevel::L5)
            // Shuffle (filtering) mode
            .shuffle(ShuffleMode::Byte)
    })
}

/// Write `arr_buf` as single chunk in `out_path`
/// uses compression of type `compressor` for output
fn write_chunk(out_path: &Path, arr_buf: Vec<u8>, compressor: &CompressionArg) -> Result<()> {
    let err_map = |_: Error| {
        RechunkingError::IoError(format!(
            "output directory at {} could not be created",
            out_path.to_string_lossy()
        ))
    };
    fs::create_dir(out_path).map_err(err_map)?;

    let compression_ctx = make_compression_ctx(compressor);
    if compression_ctx.is_none() {
        return Ok(fs::write(out_path.join("0"), arr_buf)?);
    }

    let compressed = compression_ctx.unwrap().compress(&arr_buf[..]);
    Ok(fs::write(out_path.join("0"), compressed)?)
}

/// Returns true iff `path` consists of a single normal component
fn is_single_subpath(path: &Path) -> bool {
    let comps: Vec<Component> = path.components().collect();
    comps.len() == 1 && comps.iter().all(|c| matches!(c, Component::Normal { .. }))
}

/// Returns true iff `path` consists of normal components only
fn is_subpath(path: &Path) -> bool {
    let comps: Vec<Component> = path.components().collect();
    !comps.is_empty()
        && comps[0..]
            .iter()
            .all(|c| matches!(c, Component::Normal { .. }))
}

fn main() -> Result<()> {
    let args = Args::parse();

    let top_dir = Path::new(&args.top);
    if !top_dir.is_dir() {
        return Err(RechunkingError::InvalidArgError("<TOP> must be an existing dir.").into());
    }

    let rel_in_dir = Path::new(&args.array);
    if !is_subpath(rel_in_dir) {
        return Err(RechunkingError::InvalidArgError(
            "<ARRAY> must be a subpath (no root, parent, current dir components)",
        )
        .into());
    }
    let in_dir = top_dir.join(rel_in_dir);
    if !in_dir.is_dir() {
        return Err(
            RechunkingError::InvalidArgError("<TOP>/<ARRAY> must be an existing dir").into(),
        );
    }

    let out_comp = Path::new(&args.output);
    if !is_single_subpath(out_comp) {
        return Err(RechunkingError::InvalidArgError(
            "<OUTPUT> must be a subpath (no root, parent, current dir components)
             consisting of a single component",
        )
        .into());
    }
    let rel_out_dir = rel_in_dir.parent().unwrap().join(out_comp);
    let out_dir = top_dir.join(rel_out_dir.clone());
    if out_dir.exists() {
        return Err(RechunkingError::InvalidArgError(
            "output dir exists, but overwriting is not implemented",
        )
        .into());
    }

    let metadata = parse_zarray(&in_dir)?;
    let chunks = collect_chunk_paths(&in_dir, metadata.shape)?;
    let num_chunks = chunks.len();
    write_chunk(
        &out_dir,
        concat_chunks(chunks, metadata.is_compressed)?,
        &args.compression,
    )?;
    write_metadata(
        top_dir,
        rel_in_dir,
        &rel_out_dir,
        adjust_zarray(metadata.json, num_chunks, &args.compression),
    )?;
    Ok(())
}
