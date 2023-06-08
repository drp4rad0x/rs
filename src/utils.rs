use hkdf::Hkdf;
use libsecp256k1::{util::FULL_PUBLIC_KEY_SIZE, Error as SecpError, PublicKey, SecretKey};
use rand::thread_rng;
use sha2::Sha256;

use crate::consts::EMPTY_BYTES;
use crate::types::AesKey;

#[cfg(feature = "pure")]
pub use crate::pure_aes::{symmetric_decrypt, symmetric_encrypt};

#[cfg(feature = "openssl")]
pub use crate::openssl_aes::{symmetric_decrypt, symmetric_encrypt};

#[cfg(feature = "stream")]
pub use crate::chacha20poly1305::{symmetric_encrypt, symmetric_decrypt};

/// Generate a `(SecretKey, PublicKey)` pair
pub fn generate_keypair() -> (SecretKey, PublicKey) {
    let sk = SecretKey::random(&mut thread_rng());
    (sk, PublicKey::from_secret_key(&sk))
}

/// Calculate a shared symmetric encryption key of our secret key and peer's public key by hkdf
pub fn encapsulate(sk: &SecretKey, peer_pk: &PublicKey) -> Result<AesKey, SecpError> {
    let mut shared_point = *peer_pk;
    shared_point.tweak_mul_assign(sk)?;

    let mut master = Vec::with_capacity(FULL_PUBLIC_KEY_SIZE * 2);
    master.extend(PublicKey::from_secret_key(sk).serialize().iter());
    master.extend(shared_point.serialize().iter());

    hkdf_sha256(master.as_slice())
}

/// Calculate a shared symmetric encryption key of our public key and peer's secret key by hkdf
pub fn decapsulate(pk: &PublicKey, peer_sk: &SecretKey) -> Result<AesKey, SecpError> {
    let mut shared_point = *pk;
    shared_point.tweak_mul_assign(peer_sk)?;

    let mut master = Vec::with_capacity(FULL_PUBLIC_KEY_SIZE * 2);
    master.extend(pk.serialize().iter());
    master.extend(shared_point.serialize().iter());

    hkdf_sha256(master.as_slice())
}

// private below
fn hkdf_sha256(master: &[u8]) -> Result<AesKey, SecpError> {
    let h = Hkdf::<Sha256>::new(None, master);
    let mut out = [0u8; 32];
    h.expand(&EMPTY_BYTES, &mut out)
        .map_err(|_| SecpError::InvalidInputLength)?;
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use hex::decode;

    use libsecp256k1::Error;
    use rand::{thread_rng, Rng};

    use super::*;
    use crate::consts::{AES_IV_LENGTH, EMPTY_BYTES};

    /// Remove 0x prefix of a hex string
    pub fn remove0x(hex: &str) -> &str {
        if hex.starts_with("0x") || hex.starts_with("0X") {
            return &hex[2..];
        }
        hex
    }

    /// Convert hex string to u8 vector
    pub fn decode_hex(hex: &str) -> Vec<u8> {
        decode(remove0x(hex)).unwrap()
    }

    #[test]
    fn test_remove_0x_decode_hex() {
        assert_eq!(remove0x("0x0011"), "0011");
        assert_eq!(remove0x("0X0011"), "0011");
        assert_eq!(remove0x("0011"), "0011");
        assert_eq!(decode_hex("0x0011"), [0u8, 17u8]);
    }

    #[test]
    fn test_generate_keypair() {
        let (sk1, pk1) = generate_keypair();
        let (sk2, pk2) = generate_keypair();
        assert_ne!(sk1, sk2);
        assert_ne!(pk1, pk2);
    }

    #[test]
    fn test_attempt_to_decrypt_invalid_message() {
        assert!(symmetric_decrypt(&[], &[]).is_none());

        assert!(symmetric_decrypt(&[], &[0; AES_IV_LENGTH]).is_none());
    }

    #[test]
    fn test_aes_random_key() {
        let text = b"this is a text";
        let mut key = [0u8; 32];
        thread_rng().fill(&mut key);

        assert_eq!(
            text,
            symmetric_decrypt(&key, symmetric_encrypt(&key, text).unwrap().as_slice())
                .unwrap()
                .as_slice()
        );

        let utf8_text = "😀😀😀😀".as_bytes();
        assert_eq!(
            utf8_text,
            symmetric_decrypt(&key, symmetric_encrypt(&key, utf8_text).unwrap().as_slice())
                .unwrap()
                .as_slice()
        );
    }

    #[test]
    fn test_known_symmetric_key() {
        let text = b"helloworld";
        let key = decode_hex("bd2b6b011cf6f44d9ff81731527435d19bf8ebfc5b8ec7f76a549a424a760298");
        let nonce = decode_hex("8deb4013669fd173edd3528cce6c219b7aecdda65dcf4409");
        let encrypted = decode_hex("eb916beec43b00a9f81162522181ea4d4359cd5c3184623eda10");

        let mut cipher_text = Vec::with_capacity(encrypted.len() + 24);
        cipher_text.extend(encrypted);
        cipher_text.extend(nonce);
        
        assert_eq!(text, symmetric_decrypt(&key, &cipher_text).unwrap().as_slice());
    }

    #[test]
    fn test_valid_secret() {
        // 0 < private key < group order int is valid
        let zero = [0u8; 32];
        assert_eq!(SecretKey::parse_slice(&zero).err().unwrap(), Error::InvalidSecretKey);

        let group_order_minus_1 = decode_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364140");
        SecretKey::parse_slice(&group_order_minus_1).unwrap();

        let group_order = decode_hex("fffffffffffffffffffffffffffffffebaaedce6af48a03bbfd25e8cd0364141");
        assert_eq!(
            SecretKey::parse_slice(&group_order).err().unwrap(),
            Error::InvalidSecretKey
        );
    }

    #[test]
    fn test_hkdf() {
        let text = b"secret";

        let h = Hkdf::<Sha256>::new(None, text);
        let mut out = [0u8; 32];
        let r = h.expand(&EMPTY_BYTES, &mut out);

        assert!(r.is_ok());
        assert_eq!(
            out.to_vec(),
            decode_hex("2f34e5ff91ec85d53ca9b543683174d0cf550b60d5f52b24c97b386cfcf6cbbf")
        );

        let mut two = [0u8; 32];
        let mut three = [0u8; 32];
        two[31] = 2u8;
        three[31] = 3u8;

        let sk2 = SecretKey::parse_slice(&two).unwrap();
        let pk2 = PublicKey::from_secret_key(&sk2);
        let sk3 = SecretKey::parse_slice(&three).unwrap();
        let pk3 = PublicKey::from_secret_key(&sk3);

        assert_eq!(encapsulate(&sk2, &pk3), decapsulate(&pk2, &sk3));
        assert_eq!(
            encapsulate(&sk2, &pk3).map(|v| v.to_vec()).unwrap(),
            decode_hex("6f982d63e8590c9d9b5b4c1959ff80315d772edd8f60287c9361d548d5200f82")
        );
    }
}
