use std::path::{Path, PathBuf};
use std::{io, env, fs, usize};
use thiserror::Error;

#[derive(Error, Debug)]
enum RechunkingError {
    #[error("TODO")]
    InvalidChunkFiles,
    #[error("TODO")]
    InvalidArgError,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

fn collect_chunk_files(chunks_dir: &Path, num_chunks: usize) -> Result<Vec<PathBuf>, RechunkingError> {
    // TODO: Return Path or PathBuf?
    // TODO: Should unexpected files in dir result in error too?
    let mut chunks = fs::read_dir(chunks_dir)?
        .filter_map(|entry| {
            // TODO: Additional filter predicate: path must be file.
            let path = entry.ok()?.path();
            let idx = path
                .file_name()?
                .to_str()?
                .to_string()
                .parse::<usize>()
                .ok()?;
            Some((idx, path))
        })
        .collect::<Vec<(usize, PathBuf)>>();

    // TODO: Get num_chunks from chunks vector,
    // check that it is smaller than shape (pass this as arg),
    // and that chunks are consecutive subsequence of 0..shape. 

    chunks.sort_by_key(|p| p.0);

    let (idxs, paths): (Vec<usize>, Vec<PathBuf>) = chunks.into_iter().unzip();

    // TODO: Relax this condition,
    // idxs need only be a consecutive subsequence of 0..shape
    // for non-null fill values, but same condition for null (think this part through again!)
    if !idxs.into_iter().eq(0..num_chunks) {
        return Err(RechunkingError::InvalidChunkFiles);
    }

    // TODO: Return also num_chunks, out_chunk_start_idx
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

fn write_arr(out_path: &Path, arr_buf: Vec<u8>) -> io::Result<()> {
    // TODO: Handle errors (out_path exist, cannot be created)
    fs::create_dir(out_path)?;
    // TODO: Use out_chunk_start_idx instead of 0.
    fs::write(out_path.join("0"), arr_buf)
}

fn main() -> Result<(), RechunkingError> {
    let args: Vec<String> = env::args().collect();
    // TODO: Check for args len, display help when args are invalid.
    // TODO: Check whether path is valid
    let in_dir = Path::new(&args[1]);
    // TODO: Check that arg for out_dir is single component, not path.
    let out_dir = in_dir
        .parent()
        .ok_or(RechunkingError::InvalidArgError)?
        .join(&args[2]);
    let num_chunks = args[3]
        .parse::<usize>()
        .map_err(|_| RechunkingError::InvalidArgError)?;
    let chunks = collect_chunk_files(in_dir, num_chunks);
    write_arr(&out_dir, concat_chunks(chunks?))?;
    Ok(())
}
