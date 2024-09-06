use rand::distributions::Alphanumeric;
use rand::{thread_rng, Rng, RngCore};

/// Generate random string
pub fn random_string(len: usize) -> String {
    thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

/// Random bytes
pub fn random_bytes_vec(len: usize) -> Vec<u8> {
    let mut vec = vec![0u8; len];
    thread_rng().fill_bytes(vec.as_mut_slice());
    vec
}
