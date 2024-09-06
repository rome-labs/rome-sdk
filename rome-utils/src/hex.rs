/// Convert hex string to [u64]
pub fn hex_to_u64(hex: &str) -> anyhow::Result<u64> {
    // remove the 0x
    let hex = hex.trim_start_matches("0x");
    u64::from_str_radix(hex, 16)
        .map_err(|err| anyhow::anyhow!("Failed to parse hex string to u64: {}", err))
}

/// Convert hex string to [u128]
pub fn hex_to_u128(hex: &str) -> anyhow::Result<u128> {
    // remove the 0x
    let hex = hex.trim_start_matches("0x");
    u128::from_str_radix(hex, 16)
        .map_err(|err| anyhow::anyhow!("Failed to parse hex string to u128: {}", err))
}
