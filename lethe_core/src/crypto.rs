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

// A wrapper around the raw key bytes that automatically zeroes memory on Drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct MasterKey {
    key: [u8; KEY_SIZE],
}

impl MasterKey {
    /// Create a new MasterKey from raw bytes
    pub fn new(bytes: [u8; KEY_SIZE]) -> Self {
        Self { key: bytes }
    }
    
    /// Get reference to the raw bytes (use carefully)
    pub fn as_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.key
    }
}

/// The main Crypto Engine handling encryption/decryption logic
pub struct CryptoEngine;

impl CryptoEngine {
    /// Derives a MasterKey from a password using Argon2id.
    /// Returns the Key and the Salt (salt must be stored in the index).
    pub fn derive_key(password: &str) -> Result<(MasterKey, String)> {
        let salt = SaltString::generate(&mut OsRng);
        
        // Argon2id configuration (Balanced for security/speed)
        let argon2 = Argon2::default();
        
        // Hash password to get a PHC string
        let password_hash = argon2.hash_password(password.as_bytes(), &salt)
            .map_err(|e| anyhow::anyhow!(e))?;
            
        // We extract the raw hash output to use as our ChaCha key
        // Note: In a real prod environment, we might use a KDF-specific method, 
        // but extracting the hash from Argon2 output is standard practice.
        let mut key_bytes = [0u8; KEY_SIZE];
        
        // This is a simplified extraction. 
        // For Lethe V1, we will rely on the Output Key Material (OKM) from Argon2.
        // The `password_hash` object actually contains the hash.
        let output = password_hash.hash.context("Argon2 hashing failed")?;
        
        // Ensure we copy exactly 32 bytes. 
        // If Argon2 output < 32 bytes, this is a config error.
        if output.len() < KEY_SIZE {
            return Err(anyhow::anyhow!("Argon2 output too short"));
        }
        
        key_bytes.copy_from_slice(&output.as_bytes()[..KEY_SIZE]);
        
        Ok((MasterKey::new(key_bytes), salt.as_str().to_string()))
    }

    /// Encrypts a chunk of data.
    /// Returns: (Ciphertext, Nonce)
    pub fn encrypt(data: &[u8], key: &MasterKey) -> Result<(Vec<u8>, Vec<u8>)> {
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        
        // Generate a random 192-bit (24-byte) nonce
        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = XNonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher.encrypt(nonce, data)
            .map_err(|_| anyhow::anyhow!("Encryption failure"))?;
            
        Ok((ciphertext, nonce_bytes.to_vec()))
    }

    /// Decrypts a chunk of data.
    pub fn decrypt(ciphertext: &[u8], nonce: &[u8], key: &MasterKey) -> Result<Vec<u8>> {
        if nonce.len() != NONCE_SIZE {
            return Err(anyhow::anyhow!("Invalid nonce length"));
        }
        
        let cipher = XChaCha20Poly1305::new(key.as_bytes().into());
        let nonce = XNonce::from_slice(nonce);

        let plaintext = cipher.decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("Decryption failure or Auth Tag mismatch"))?;
            
        Ok(plaintext)
    }
}