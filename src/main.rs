use anyhow::Result;
use blosc::{Clevel, Compressor, Context, ShuffleMode};
use clap::Parser;
use json::JsonValue;
use rayon::prelude::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
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

#[derive(Error, Debug)]
enum RechunkingError {
    #[error("{0} is not a valid JSON file")]
    InvalidJSON(String),

    #[error("Zarr metadata file {0} is invalid: {1}")]
    InvalidMetadataFile(String, &'static str),

    #[error("Invalid chunk files in folder {0}: {1}")]
    InvalidChunkFiles(String, &'static str),

    #[error("Invalid argument command line argument passed: {0}")]
    InvalidArgError(&'static str),

    #[error("Decompression failed")]
    DecompressionError,

    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

struct Metadata {
    json: JsonValue,
    shape: usize,
    is_compressed: bool,
}

fn parse_zarray(in_dir: &Path) -> Result<Metadata> {
    let zarray_file = in_dir.join(".zarray");
    let zarray_file_str = zarray_file.to_string_lossy().to_string();
    let json = json::parse(&fs::read_to_string(zarray_file.as_path())?)
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
        // NOTE: compressor options are not validated in detail
        true
    };

    Ok(Metadata {
        json,
        shape,
        is_compressed,
    })
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

fn collect_chunks(chunks_dir: &Path, shape: usize) -> Result<Vec<PathBuf>> {
    // TODO: Return Path or PathBuf?
    let chunks_dir_str = chunks_dir.to_string_lossy().to_string();
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
        return Err(RechunkingError::InvalidChunkFiles(
            chunks_dir_str,
            "found too many chunks for given shape",
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

fn write_chunk(out_path: &Path, arr_buf: Vec<u8>) -> Result<()> {
    // TODO: Handle errors (out_path exist, cannot be created)
    fs::create_dir(out_path)?;

    let context = Context::new()
        .blocksize(None)
        .compressor(Compressor::LZ4)
        .unwrap()
        .clevel(Clevel::L5)
        .shuffle(ShuffleMode::Byte);

    // TODO: Check why compression has practically no effect on merged chunk filesize
    let compressed = context.compress(&arr_buf[..]);

    Ok(fs::write(out_path.join("0"), compressed)?)
}

fn is_normal_comp(path: &Path) -> bool {
    let comps: Vec<Component> = path.components().collect();
    comps.len() == 1 && comps.iter().all(|c| matches!(c, Component::Normal { .. }))
}

fn main() -> Result<()> {
    // TODO: Documentation:
    // * functions
    // * cli parameters
    // * help message
    // * README.md
    let args = Args::parse();
    let top_dir = Path::new(&args.top);
    if !top_dir.is_dir() {
        return Err(RechunkingError::InvalidArgError("<TOP> must be existing dir.").into());
    }
    // TODO: Check that rel_in_dir has only normal components and separators (no root, ..)
    let rel_in_dir = Path::new(&args.array);
    let in_dir = top_dir.join(rel_in_dir);
    if !in_dir.is_dir() {
        return Err(RechunkingError::InvalidArgError("<TOP>/<ARRAY> must be existing dir").into());
    }
    let out_comp = Path::new(&args.output);
    if !is_normal_comp(out_comp) {
        return Err(RechunkingError::InvalidArgError("<OUTPUT> must be a single component").into());
    }
    let rel_out_dir = rel_in_dir.parent().unwrap().join(out_comp);
    let out_dir = top_dir.join(rel_out_dir.clone());
    // TODO: Handle case that out_dir exits or is equal to array_dir (forbid overwriting?!)
    let metadata = parse_zarray(&in_dir)?;
    let chunks = collect_chunks(&in_dir, metadata.shape)?;
    let num_chunks = chunks.len();
    write_chunk(&out_dir, concat_chunks(chunks, metadata.is_compressed)?)?;
    write_metadata(
        top_dir,
        rel_in_dir,
        &rel_out_dir,
        adjust_zarray(metadata.json, num_chunks),
    )?;
    Ok(())
}
