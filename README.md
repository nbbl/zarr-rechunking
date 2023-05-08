This is an implementation of a Rust program for rechunking an one dimensional Zarr array with one data point per chunk into an array consisting of a single chunk. It was implemented as a case study for my application as backend engineer at ConstellR.

# Challenge Description
This was the initial description of the case study. The requirements were further refined after an e-mail exchange with one of ConstellR's engineers, Igor Khomyakov. See [the section on requirements below](#requirements).

> **Rechunk Zarr variable in-place**
> 
> You are given a Zarr dataset with a one-dimensional variable that has N data points and chunked one data point per chunk. Implement a _program_ that rechunks this variable to contain N data points per chunk, i.e. merge all data points into one chunk. This rechunking must be implemented _in-place_, i.e. you cannot read the dataset and then produce a new dataset with a rechunked variable.
> 
> To earn extra points, consider implementing this using Rust programming language without help of any existing Zarr library.

# Usage of Program

## Building
The program is built with cargo. It suffices to exectue `cargo build` in a shell. Note that you might need to install `libblosc-dev` (Ubuntu Linux package) for compression and decompression of chunks. 

## CLI
After building the program is run with `cargo run -- <args>`. Use `--help` for a description of the arguments.

# Requirements
The following requirements for the implementation follow either directly from the [challenge description](#challenge-description) or were gathered in the discussion with Igor Khomyakov.

## Basics
- **Terminology** From our discussion and the sample data the following definitions follow:
	- data set = one or multiple Zarr arrays grouped together in a folder structure
	- variable = single Zarr array representing a measured value
	- data point = single datum at an array index.
- **Storage** Zarr can be accessed via arbitrary APIs, e.g. S3 buckets or HTTP URLs. This program is implemented using file-based storage (POSIX) only.
- **Definition of in-place** The in-place requirement refers to a local change of one variable inside of a data set without rewriting the whole data set. This limitation is not to be confused with the definition of in-place refering to an algorithm which transforms input using no auxiliary data structure. 
- **Supported Zarr Features** Handling of the example data set requires the implementation of grouping, compression/decompression, and filtering (as part of compression/decompression).
- **Zarr version** Version 2 is the latest stable version, so the solution is implemented to be compatible with this version.

## Functional Capabilities and Limitations
- **Data types** The program only supports processing of fixed sized types, as the logic for variable size object arrays would have been too complex to implement in the given time. However, it is not limited to simple types, i.e. the program can also handle structured data types.
	- **No checking of chunk byte size** It was requested to not spend additional time on the logic of handling simple vs. structured types, but the parsing of structured chunk byte sizes from metadata would have incurred such additional effort.
- **Fill values** Filling gaps of the merged array with fill values was not implemented, because generating them for all compatible types from the metadata would have incurred additional time to implement, which Igor recommended against. This leads to two limitations:
	- **No gaps** The indices of the chunks must be consecutive.
	- **Start at index 0** Chunk files of input must start at index 0, so that the merged chunk is aligned with chunk tiling borders. 
	Gaps at the end of the array, i.e., from end of merged chunk to the maximum index, can exists and will be interpreted as being padded with the fill value. This does not require additional adaptation of the data.
- **Compression/Decompression** Compression and decompression were implemented for Blosc as the primary compressor with the option to use different internal compressors. See `--compression` option of help message for a list of internal compressors. The block size is determined automatically, for the output the compression level is set to 5, and the shuffle (filtering) mode to byte-shuffle. 
- **Validation** The metadata from the `.zarray` file (only of the variable that is being modified), and the range of chunk files (see **Fill values**) are validated for the input. If a `.zmetadata` file exists in the top dir, a key for the output is generetad in it. No validation of chunk byte sizes (due to limitation of **Data types**, see above) is performed.

## Additional Points for Consideration
Additionally to the functional requirements the following two points were considered: 
- **Testing** The program was tested manually using the Python Zarr package. One basic test is the equivalence of arrays before and after rechunking. An option for automatic testing would be property based testing according to the following schema:
    - Pseudo-randomly create Python Numpy arrays.
    - Serialize arrays as Zarr arrays with chunk size 1.
    - Rechunk serialized arrays with this program.
    - Deserialize arrays as Zarr arrays and verify equivalence with original.
- **Parallel processing** The challenge description did not ask to implement parallel processing, but as the parallel reading and decompression of files was an easy way to improve runtime performance, and straightforward to implement in Rust, I added this. The used Blosc compression/decompression library is optimized using multi-threading. 

# Resources
These are some resources that were handy during the implementation.

## Zarr
- [Stable (v2) spec](https://zarr.readthedocs.io/en/stable/spec/v2.html)

## Rust
- [The Rust Programming Language - e-book from Rust Project Developers](https://web.mit.edu/rust-lang_v1.25/arch/amd64_ubuntu1404/share/doc/rust/html/book/)
- [Rust Intro for C++ Programmers](https://github.com/nrc/r4cppp)
- [Rust Cookbook - Data Parallelism](https://rust-lang-nursery.github.io/rust-cookbook/concurrency/parallel.html)
- [Blosc (block-oriented compression library)](https://docs.rs/blosc/latest/blosc/)

## Other
- [Wikipedia: In-place algorithm](https://en.wikipedia.org/wiki/In-place_algorithm)