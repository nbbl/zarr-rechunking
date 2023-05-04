use std::path::{Path, PathBuf};
use std::{env, fs, usize};
use thiserror::Error;

#[derive(Error, Debug)]
enum FindError {
    #[error("TODO")]
    InvalidChunkFiles,
    #[error(transparent)]
    IoError(#[from] std::io::Error),
}

fn collect_chunk_files(chunks_dir: &Path, num_chunks: usize) -> Result<Vec<PathBuf>, FindError> {
    // TODO: Return Path or PathBuf?
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

    chunks.sort_by_key(|p| p.0);

    let (idxs, paths): (Vec<usize>, Vec<PathBuf>) = chunks.into_iter().unzip();

    if !idxs.into_iter().eq(0..num_chunks) {
        return Err(FindError::InvalidChunkFiles);
    }

    Ok(paths)
}

fn main() -> Result<(), FindError> {
    let args: Vec<String> = env::args().collect();
    // TODO: Check for args len.
    let chunks_dir = Path::new(&args[1]);
    let chunks = collect_chunk_files(chunks_dir, 682);
    println!("{:?}", chunks);
    Ok(())
}
