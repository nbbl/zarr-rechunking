use json::JsonValue;
use std::path::{Path, PathBuf};
use std::{env, fs, io, usize};
use thiserror::Error;

#[derive(Error, Debug)]
enum RechunkingError {
    // TODO: different error types (metadata, chunks, ...)?
    #[error("TODO")]
    IncompatibleZarrVersion,
    #[error("TODO")]
    IncompatibleChunkSize,
    #[error("TODO")]
    IncompatibleArrayShape,
    #[error("TODO")]
    InvalidJSON,
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

fn parse_zarray(json_file: &Path) -> Result<Metadata, RechunkingError> {
    let json =
        json::parse(&fs::read_to_string(json_file)?).map_err(|_| RechunkingError::InvalidJSON)?;
    if json["zarr_format"] != 2 {
        return Err(RechunkingError::IncompatibleZarrVersion);
    }
    if json["chunks"].members().as_slice() != [1] {
        return Err(RechunkingError::IncompatibleChunkSize);
    }
    let [shapes] = json["shape"].members().as_slice() else {
        return Err(RechunkingError::IncompatibleArrayShape)
    };
    let shape = shapes
        .as_usize()
        .ok_or(RechunkingError::IncompatibleArrayShape)?;
    Ok(Metadata { json, shape })
}

fn collect_chunk_files(chunks_dir: &Path, shape: usize) -> Result<Vec<PathBuf>, RechunkingError> {
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
    // TODO: Implement decompression
    // TODO: Implement parallel processing
    // TODO: Error handling in this function?
    paths
        .iter()
        .flat_map(|p| fs::read(p.as_path()))
        .flatten()
        .collect()
}

fn write_chunk(out_path: &Path, arr_buf: Vec<u8>) -> io::Result<()> {
    // TODO: Handle errors (out_path exist, cannot be created)
    fs::create_dir(out_path)?;
    // TODO: Implement compression.
    fs::write(out_path.join("0"), arr_buf)
}

fn main() -> Result<(), RechunkingError> {
    let args: Vec<String> = env::args().collect();
    // TODO: Check for args len, display help when args are invalid.
    // TODO: Check whether path is valid
    let in_dir = Path::new(&args[1]);
    // TODO: Check that arg for out_dir is single component, not path.
    let shape = parse_zarray(in_dir.join(".zarray").as_path())?.shape;
    println!("Array shape: {:?}", shape);
    let out_dir = in_dir
        .parent()
        .ok_or(RechunkingError::InvalidArgError)?
        .join(&args[2]);
    // TODO: Get shape from .zarray
    let shape = args[3]
        .parse::<usize>()
        .map_err(|_| RechunkingError::InvalidArgError)?;
    let chunks = collect_chunk_files(in_dir, shape)?;
    write_chunk(&out_dir, concat_chunks(chunks))?;
    // write_zarray(&out_dir, adjust_metadata());
    Ok(())
}
