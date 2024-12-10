use ethers::types::{Transaction, H256};
use flate2::{bufread::ZlibDecoder, write::ZlibEncoder, Compression};
use rlp::Decodable;
use sha3::{Digest, Keccak256, Sha3_256};
use std::io::{Read, Write};

use super::{types::DaSubmissionBlock, BlobParam};

pub fn string_to_base64_namespace(input: &str) -> anyhow::Result<String> {
    // Convert the input string to a hexadecimal string
    let hex_string = hex::encode(input);

    // Ensure the hex string is exactly 10 bytes (20 hex characters) long
    let user_id_hex = if hex_string.len() >= 20 {
        hex_string[..20].to_string()
    } else {
        format!("{:0>20}", hex_string) // Pad with zeros in the most significant bytes
    };

    // Construct the namespace: version (1 byte) + 18 bytes of 0 + 10 bytes of user-specified data
    let mut namespace_bytes = vec![0u8; 29];
    namespace_bytes[0] = 0; // Version 0

    // Convert user-specified part to bytes and place them in the least significant 10 bytes
    let user_id_bytes = hex::decode(user_id_hex)?;
    namespace_bytes[19..].copy_from_slice(&user_id_bytes);

    // Encode the namespace bytes to base64
    let base64_namespace = base64::encode(&namespace_bytes);
    Ok(base64_namespace)
}

pub fn create_new_commitment(hashed_old_commitment: H256, tx_signature: H256) -> H256 {
    // Convert H256 values to byte arrays
    let old_commit_bytes = hashed_old_commitment.as_bytes();
    let input_tx_bytes = tx_signature.as_bytes();

    // Combine the byte arrays
    let mut combined = Vec::new();
    combined.extend_from_slice(old_commit_bytes);
    combined.extend_from_slice(input_tx_bytes);

    // Hash the combined array with Keccak256
    let mut hasher = Keccak256::new();
    hasher.update(combined);
    let result = hasher.finalize();

    // Convert the result back to H256
    H256::from_slice(&result)
}

pub fn derive_commitment(data: &String) -> String {
    // Create a new SHA3-256 hasher instance
    let mut hasher = Sha3_256::new();

    // Update the hasher with the input data
    hasher.update(data.as_bytes());

    // Finalize the hash and retrieve the result
    let result = hasher.finalize();

    // Encode the result as a base64 string
    base64::encode(result)
}

pub fn compress_data(streamout: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&streamout)?;
    let compressed_data = encoder.finish()?;
    Ok(compressed_data)
}

pub fn decompress_data(compressed_data: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(&compressed_data[..]);
    let mut decompressed_data = Vec::new();
    decoder.read_to_end(&mut decompressed_data)?;
    Ok(decompressed_data)
}

pub fn compress_blocks_to_blobs(
    namespace: &str,
    blocks: &[DaSubmissionBlock],
) -> anyhow::Result<Vec<BlobParam>> {
    let mut blobs: Vec<BlobParam> = vec![];

    let mut index = 0;
    for block in blocks.iter() {
        for transaction in block.transactions.iter() {
            let tx_data = compress_data(transaction.rlp().0.to_vec())?;
            let data = base64::encode(&tx_data);

            blobs.push(BlobParam {
                namespace: namespace.to_string(),
                data: data.clone(),
                share_version: 0,
                commitment: derive_commitment(&data),
                index,
            });

            index += 1;
        }
    }

    Ok(blobs)
}

pub fn decode_blob_to_tx(blob: &BlobParam) -> anyhow::Result<Transaction> {
    let data = base64::decode(&blob.data)?;
    let tx_data = decompress_data(data)?;
    let tx_rlp = rlp::Rlp::new(&tx_data);
    let tx = Transaction::decode(&tx_rlp)?;

    Ok(tx)
}
