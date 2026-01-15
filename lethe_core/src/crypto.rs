use chacha20poly1305::{
    aead::{Aead, KeyInit},
    XChaCha20Poly1305, XNonce
};
use argon2::{
    password_hash::{rand_core::OsRng, SaltString},
    Argon2, PasswordHasher
};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};
use anyhow::{Result, Context};

const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 24;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    key: [u8; KEY_SIZE],
}

impl MasterKey {
    pub fn new(bytes: [u8; KEY_SIZE]) -> Self {
        Self { key: bytes }
    }
    
    pub fn as_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.key
    }
}

pub struct CryptoEngine;

impl CryptoEngine {
    /// Generates a NEW salt and derives a key (For "Init")
    pub fn derive_key(password: &str) -> Result<(MasterKey, String)> {
        let salt = SaltString::generate(&mut OsRng);
        Self::derive_internal(password, &salt)
    }

    /// Uses an EXISTING salt to derive the key (For "Unlock")
    pub fn derive_key_with_salt(password: &str, salt_str: &str) -> Result<(MasterKey, String)> {
        let salt = SaltString::from_b64(salt_str)
            .map_err(|e| anyhow::anyhow!("Invalid salt format: {}", e))?;
        Self::derive_internal(password, &salt)
    }

    fn derive_internal(password: &str, salt: &SaltString) -> Result<(MasterKey, String)> {
        let argon2 = Argon2::default();
        let password_hash = argon2.hash_password(password.as_bytes(), salt)
            .map_err(|e| anyhow::anyhow!(e))?;

        let output = password_hash.hash.context("Argon2 hashing failed")?;
        
        if output.len() < KEY_SIZE {
            return Err(anyhow::anyhow!("Argon2 output too short"));
        }
        
        let mut key_bytes = [0u8; KEY_SIZE];
        key_bytes.copy_from_slice(&output.as_bytes()[..KEY_SIZE]);
        
        Ok((MasterKey::new(key_bytes), salt.as_str().to_string()))
    }

    pub fn encrypt(data: &[u8], key: &MasterKey) -> Result<(Vec<u8>, Vec<u8>)> {
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        let ciphertext = cipher.encrypt(nonce, data)
            .map_err(|_| anyhow::anyhow!("Encryption failure"))?;
        
        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    pub fn decrypt(ciphertext: &[u8], nonce: &[u8], key: &MasterKey) -> Result<Vec<u8>> {
        if nonce.len() != NONCE_SIZE {
            return Err(anyhow::anyhow!("Invalid nonce length"));
        }
        
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        let nonce = XNonce::from_slice(nonce);

        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Decryption failed (Wrong password or corrupted data)"))?;
        
        Ok(plaintext)
    }
}