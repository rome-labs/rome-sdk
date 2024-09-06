use ethnum::U256;

/// Append a `U256` to an RLP stream.
#[inline]
pub fn u256_rlp_append(this: &U256, s: &mut rlp::RlpStream) {
    let leading_empty_bytes = (this.leading_zeros() as usize) / 8;
    let buffer = &this.to_be_bytes()[leading_empty_bytes..];
    s.append(&buffer);
}

/// Decode a `U256` from an RLP stream.
#[inline]
pub fn u256(rlp: &rlp::Rlp) -> Result<U256, rlp::DecoderError> {
    rlp.decoder().decode_value(|bytes| {
        if !bytes.is_empty() && bytes[0] == 0 {
            Err(rlp::DecoderError::RlpInvalidIndirection)
        } else if bytes.len() <= 32 {
            let mut buffer = [0_u8; 32];
            buffer[(32 - bytes.len())..].copy_from_slice(bytes);
            Ok(U256::from_be_bytes(buffer))
        } else {
            Err(rlp::DecoderError::RlpIsTooBig)
        }
    })
}
