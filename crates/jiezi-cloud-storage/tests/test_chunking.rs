//! Integration tests for CDC chunking.

use jiezi_cloud_storage::chunking::FastCdcChunker;
use jiezi_cloud_storage::hashing::sha256_hex;

fn make_data(n: usize) -> Vec<u8> {
    // Pseudo-random but deterministic data
    (0..n).map(|i| (i.wrapping_mul(6364136223846793005) >> 56) as u8).collect()
}

#[test]
fn test_chunk_empty_input() {
    let chunker = FastCdcChunker::default();
    let chunks = chunker.chunk(b"");
    assert!(chunks.is_empty(), "empty input should produce zero chunks");
}

#[test]
fn test_chunk_small_input_single_chunk() {
    // Data smaller than min_size must still produce exactly one chunk
    let chunker = FastCdcChunker::new(512, 4096, 16384);
    let data = b"too small to split";
    let chunks = chunker.chunk(data);
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].offset, 0);
    assert_eq!(chunks[0].size as usize, data.len());
}

#[test]
fn test_chunks_cover_entire_input() {
    let chunker = FastCdcChunker::default();
    let data = make_data(200 * 1024); // 200 KiB
    let chunks = chunker.chunk(&data);

    assert!(!chunks.is_empty());

    // Verify total coverage
    let total_bytes: usize = chunks.iter().map(|c| c.size as usize).sum();
    assert_eq!(total_bytes, data.len(), "chunks must cover entire input");
}

#[test]
fn test_chunks_are_contiguous() {
    let chunker = FastCdcChunker::default();
    let data = make_data(128 * 1024); // 128 KiB
    let chunks = chunker.chunk(&data);

    let mut expected_offset = 0u64;
    for c in &chunks {
        assert_eq!(c.offset, expected_offset, "chunks must be contiguous");
        expected_offset += u64::from(c.size);
    }
}

#[test]
fn test_chunks_reconstruct_original() {
    let chunker = FastCdcChunker::default();
    let data = make_data(256 * 1024); // 256 KiB
    let chunks = chunker.chunk(&data);

    let reconstructed: Vec<u8> = chunks.iter().flat_map(|c| c.data.iter().copied()).collect();
    assert_eq!(reconstructed, data, "concatenating chunks must restore original data");
}

#[test]
fn test_chunk_hashes_match_data() {
    let chunker = FastCdcChunker::default();
    let data = make_data(64 * 1024);
    let chunks = chunker.chunk(&data);

    for c in &chunks {
        let expected = sha256_hex(&c.data);
        assert_eq!(c.hash, expected, "each chunk's hash must match its data");
    }
}

#[test]
fn test_chunking_is_deterministic() {
    let chunker = FastCdcChunker::default();
    let data = make_data(128 * 1024);
    let run1 = chunker.chunk(&data);
    let run2 = chunker.chunk(&data);

    assert_eq!(run1.len(), run2.len(), "chunk count must be deterministic");
    for (a, b) in run1.iter().zip(run2.iter()) {
        assert_eq!(a.offset, b.offset);
        assert_eq!(a.size, b.size);
        assert_eq!(a.hash, b.hash);
    }
}

#[test]
fn test_cdc_property_prefix_change_is_local() {
    // Prepend 1 byte.  Due to CDC, only the first few chunk boundaries
    // should shift; chunks in the middle and end should stabilise.
    let chunker = FastCdcChunker::new(256, 1024, 4096);
    let data = make_data(48 * 1024);

    let chunks_original = chunker.chunk(&data);

    let mut modified = data.clone();
    modified.insert(0, 0xFF); // Insert 1 byte at the beginning
    let chunks_modified = chunker.chunk(&modified);

    // Collect hashes of the original chunks
    let original_hashes: std::collections::HashSet<&str> =
        chunks_original.iter().map(|c| c.hash.as_str()).collect();
    let modified_hashes: std::collections::HashSet<&str> =
        chunks_modified.iter().map(|c| c.hash.as_str()).collect();

    // The majority of chunks should be identical (CDC stability property).
    // We expect at least 60% of original chunks to survive unchanged.
    let shared_count = original_hashes.intersection(&modified_hashes).count();
    let min_expected = original_hashes.len() * 60 / 100;
    assert!(
        shared_count >= min_expected,
        "FastCDC must preserve most chunks on a small prefix change: \
        shared={shared_count}/{} expected>={min_expected}",
        original_hashes.len()
    );
}
