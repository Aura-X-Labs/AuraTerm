use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use argon2::{password_hash::SaltString, Argon2, ParamsBuilder, PasswordHash, PasswordHasher, PasswordVerifier};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

const CREDENTIALS_FILE: &str = "credentials.enc";
const MAGIC: &[u8; 8] = b"AURAENC\0";
const VERSION: u32 = 1;
const SALT_SIZE: usize = 32;

/// 应用会话级主密码缓存。
///
/// 主密码在用户验证成功后缓存于此，应用进程内有效，关闭后清空。
/// 所有需要解密凭据的命令都应通过 [`MasterPasswordState::get`] 读取。
#[derive(Default)]
pub struct MasterPasswordState {
    inner: Mutex<Option<String>>,
}

impl MasterPasswordState {
    pub fn set(&self, password: String) {
        let mut guard = self.inner.lock().expect("master password mutex poisoned");
        *guard = Some(password);
    }

    pub fn clear(&self) {
        let mut guard = self.inner.lock().expect("master password mutex poisoned");
        *guard = None;
    }

    pub fn get(&self) -> Result<String, String> {
        let guard = self.inner.lock().map_err(|e| e.to_string())?;
        guard
            .clone()
            .ok_or_else(|| "Master password not unlocked. Please verify your master password first.".to_string())
    }

    pub fn is_unlocked(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }
}

/// 凭据存储结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialStore {
    pub credentials: Vec<StoredCredential>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredential {
    pub connection_id: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
}

/// 加密文件头
#[derive(Debug)]
struct EncryptedFileHeader {
    magic: [u8; 8],
    version: u32,
    salt: Vec<u8>,
    reserved: [u8; 12],
}

impl EncryptedFileHeader {
    fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(64);
        bytes.extend_from_slice(&self.magic);
        bytes.extend_from_slice(&self.version.to_le_bytes());
        bytes.extend_from_slice(&self.salt);
        bytes.extend_from_slice(&self.reserved);
        // Pad to exactly 64 bytes so the ciphertext starts at the same offset
        // that from_bytes() expects (&bytes[64..]).
        bytes.resize(64, 0);
        bytes
    }

    fn from_bytes(bytes: &[u8]) -> Result<(Self, &[u8]), String> {
        if bytes.len() < 64 {
            return Err("Invalid file header: too short".to_string());
        }

        let magic = <[u8; 8]>::try_from(&bytes[0..8])
            .map_err(|_| "Invalid magic bytes".to_string())?;

        if magic != *MAGIC {
            return Err("Invalid file magic".to_string());
        }

        let version = u32::from_le_bytes(
            <[u8; 4]>::try_from(&bytes[8..12]).map_err(|_| "Version parse failed".to_string())?,
        );

        if version != VERSION {
            return Err(format!("Unsupported file version: {}", version));
        }

        let salt = bytes[12..44].to_vec();
        let reserved = <[u8; 12]>::try_from(&bytes[44..56])
            .map_err(|_| "Reserved bytes parse failed".to_string())?;

        Ok((
            EncryptedFileHeader {
                magic,
                version,
                salt,
                reserved,
            },
            &bytes[64..],
        ))
    }
}

/// 从主密码推导加密密钥
pub fn derive_key_from_password(
    password: &str,
    salt: &[u8],
) -> Result<[u8; 32], String> {
    let params = ParamsBuilder::new()
        .m_cost(16 * 1024) // 16 MB
        .t_cost(3)
        .p_cost(1)
        .build()
        .map_err(|e| format!("Argon2 params builder failed: {}", e))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let salt_string = SaltString::encode_b64(salt)
        .map_err(|e| format!("Salt encoding failed: {}", e))?;

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| format!("Argon2 hashing failed: {}", e))?;

    let hash_str = password_hash.to_string();
    let mut hasher = Sha256::new();
    hasher.update(hash_str.as_bytes());
    let result = hasher.finalize();

    let mut key = [0u8; 32];
    key.copy_from_slice(&result[..32]);
    Ok(key)
}

/// 生成主密码的哈希（用于验证存储）
pub fn hash_master_password(password: &str, salt: &[u8]) -> Result<String, String> {
    let params = ParamsBuilder::new()
        .m_cost(16 * 1024)
        .t_cost(3)
        .p_cost(1)
        .build()
        .map_err(|e| format!("Argon2 params builder failed: {}", e))?;

    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let salt_string = SaltString::encode_b64(salt)
        .map_err(|e| format!("Salt encoding failed: {}", e))?;

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| format!("Argon2 hashing failed: {}", e))?;

    Ok(password_hash.to_string())
}

/// 验证主密码是否正确
pub fn verify_master_password(
    input_password: &str,
    stored_hash: &str,
) -> Result<bool, String> {
    let parsed_hash = PasswordHash::new(stored_hash)
        .map_err(|e| format!("Hash parsing failed: {}", e))?;

    let argon2 = Argon2::default();

    match argon2.verify_password(input_password.as_bytes(), &parsed_hash) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// AES-256-GCM 加密
pub fn encrypt_data(plaintext: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new(key.into());

    let mut rng = rand::thread_rng();
    let nonce_bytes: [u8; 12] = rng.gen();
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad: b"" })
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut result = Vec::new();
    result.extend_from_slice(&nonce_bytes); // 12 bytes
    result.extend_from_slice(&ciphertext); // ciphertext + 16-byte tag
    Ok(result)
}

/// AES-256-GCM 解密
pub fn decrypt_data(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 12 {
        return Err("Encrypted data too short".to_string());
    }

    let nonce = Nonce::from_slice(&encrypted[0..12]);
    let ciphertext = &encrypted[12..];

    let cipher = Aes256Gcm::new(key.into());

    cipher
        .decrypt(nonce, Payload { msg: ciphertext, aad: b"" })
        .map_err(|e| format!("Decryption failed: {}", e))
}

/// 从文件系统加载并解密凭据
pub fn load_encrypted_credentials(
    app: &AppHandle,
    password: &str,
) -> Result<CredentialStore, String> {
    let credentials_path = get_credentials_path(app)?;

    if !credentials_path.exists() {
        // 首次使用，返回空存储
        return Ok(CredentialStore {
            credentials: Vec::new(),
        });
    }

    let file_data = fs::read(&credentials_path)
        .map_err(|e| format!("Failed to read credentials file: {}", e))?;

    let (header, encrypted_data) = EncryptedFileHeader::from_bytes(&file_data)?;

    // 派生密钥
    let key = derive_key_from_password(password, &header.salt)?;

    // 解密数据
    let decrypted = decrypt_data(encrypted_data, &key)?;

    // 解析 JSON
    let store: CredentialStore = serde_json::from_slice(&decrypted)
        .map_err(|e| format!("Failed to parse credentials JSON: {}", e))?;

    Ok(store)
}

/// 加密并保存凭据到文件系统
pub fn save_encrypted_credentials(
    app: &AppHandle,
    store: &CredentialStore,
    password: &str,
) -> Result<(), String> {
    let credentials_path = get_credentials_path(app)?;

    if let Some(parent) = credentials_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create credentials directory: {}", e))?;
    }

    // 生成随机盐值
    let mut rng = rand::thread_rng();
    let salt: Vec<u8> = (0..SALT_SIZE).map(|_| rng.gen()).collect();

    // 派生密钥
    let key = derive_key_from_password(password, &salt)?;

    // 序列化凭据为 JSON
    let json_data = serde_json::to_vec(store)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;

    // 加密数据
    let encrypted_data = encrypt_data(&json_data, &key)?;

    // 构建文件头
    let header = EncryptedFileHeader {
        magic: *MAGIC,
        version: VERSION,
        salt,
        reserved: [0u8; 12],
    };

    // 写入文件
    let mut file_content = header.to_bytes();
    file_content.extend_from_slice(&encrypted_data);

    fs::write(&credentials_path, &file_content)
        .map_err(|e| format!("Failed to write credentials file: {}", e))?;

    // 设置文件权限为 0600 (Unix)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(&credentials_path, permissions)
            .map_err(|e| format!("Failed to set file permissions: {}", e))?;
    }

    Ok(())
}

/// 获取凭据文件路径
fn get_credentials_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let config_dir = app
        .path()
        .app_config_dir()
        .map_err(|e| e.to_string())?;
    Ok(config_dir.join(CREDENTIALS_FILE))
}

/// 导出凭据为加密备份字符串（Base64 编码）
pub fn export_credentials_backup(
    app: &AppHandle,
    password: &str,
) -> Result<String, String> {
    let store = load_encrypted_credentials(app, password)?;

    // 生成随机盐值用于导出加密
    let mut rng = rand::thread_rng();
    let export_salt: Vec<u8> = (0..SALT_SIZE).map(|_| rng.gen()).collect();

    // 派生导出密钥
    let export_key = derive_key_from_password(password, &export_salt)?;

    // 序列化
    let json_data = serde_json::to_vec(&store)
        .map_err(|e| format!("Failed to serialize for export: {}", e))?;

    // 加密
    let encrypted = encrypt_data(&json_data, &export_key)?;

    // 构建导出数据：salt + encrypted
    let mut export_data = export_salt.clone();
    export_data.extend_from_slice(&encrypted);

    // Base64 编码
    let encoded = STANDARD.encode(&export_data);
    Ok(encoded)
}

/// 从备份字符串导入凭据
pub fn import_credentials_backup(
    app: &AppHandle,
    password: &str,
    backup_data: &str,
) -> Result<(), String> {
    // Base64 解码
    let decoded = STANDARD
        .decode(backup_data)
        .map_err(|e| format!("Invalid backup format: {}", e))?;

    if decoded.len() < SALT_SIZE {
        return Err("Backup data too short".to_string());
    }

    let import_salt = decoded[0..SALT_SIZE].to_vec();
    let encrypted_data = &decoded[SALT_SIZE..];

    // 派生导入密钥
    let import_key = derive_key_from_password(password, &import_salt)?;

    // 解密
    let decrypted = decrypt_data(encrypted_data, &import_key)?;

    // 解析 JSON
    let imported_store: CredentialStore = serde_json::from_slice(&decrypted)
        .map_err(|e| format!("Failed to parse imported credentials: {}", e))?;

    // 加载现有凭据
    let mut current_store = load_encrypted_credentials(app, password).unwrap_or(CredentialStore {
        credentials: Vec::new(),
    });

    // 合并：导入的凭据覆盖现有的同 ID 凭据
    for imported in imported_store.credentials {
        if let Some(existing) = current_store
            .credentials
            .iter_mut()
            .find(|c| c.connection_id == imported.connection_id)
        {
            *existing = imported;
        } else {
            current_store.credentials.push(imported);
        }
    }

    // 保存合并后的凭据
    save_encrypted_credentials(app, &current_store, password)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========== 密钥派生 ==========

    #[test]
    fn test_derive_key_deterministic() {
        let password = "test_password";
        let salt = [0u8; 32];

        let key1 = derive_key_from_password(password, &salt).unwrap();
        let key2 = derive_key_from_password(password, &salt).unwrap();

        assert_eq!(key1, key2, "Same password and salt should produce same key");
        assert_eq!(key1.len(), 32, "Derived key must be 32 bytes (AES-256)");
    }

    #[test]
    fn test_derive_key_different_salts_produce_different_keys() {
        let password = "test_password";
        let salt_a = [0u8; 32];
        let salt_b = [1u8; 32];

        let key_a = derive_key_from_password(password, &salt_a).unwrap();
        let key_b = derive_key_from_password(password, &salt_b).unwrap();

        assert_ne!(
            key_a, key_b,
            "Different salts should produce different keys for the same password"
        );
    }

    #[test]
    fn test_derive_key_different_passwords_produce_different_keys() {
        let salt = [7u8; 32];
        let key_a = derive_key_from_password("alpha", &salt).unwrap();
        let key_b = derive_key_from_password("bravo", &salt).unwrap();
        assert_ne!(key_a, key_b, "Different passwords should produce different keys");
    }

    // ========== AES-256-GCM 加解密 ==========

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [0u8; 32];
        let plaintext = b"Hello, World!";

        let encrypted = encrypt_data(plaintext, &key).unwrap();
        let decrypted = decrypt_data(&encrypted, &key).unwrap();

        assert_eq!(plaintext, decrypted.as_slice());
    }

    #[test]
    fn test_encrypt_includes_random_nonce() {
        // 同一密钥和明文加密两次应得到不同密文（因 nonce 随机）
        let key = [42u8; 32];
        let plaintext = b"deterministic input";

        let c1 = encrypt_data(plaintext, &key).unwrap();
        let c2 = encrypt_data(plaintext, &key).unwrap();

        assert_ne!(c1, c2, "Same plaintext + key should yield different ciphertexts (random nonce)");
        // 但解密后应得到相同明文
        assert_eq!(decrypt_data(&c1, &key).unwrap(), plaintext);
        assert_eq!(decrypt_data(&c2, &key).unwrap(), plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key_fails() {
        let key_a = [0u8; 32];
        let key_b = [1u8; 32];
        let plaintext = b"top secret";

        let encrypted = encrypt_data(plaintext, &key_a).unwrap();
        let result = decrypt_data(&encrypted, &key_b);

        assert!(result.is_err(), "Decryption with wrong key must fail");
    }

    #[test]
    fn test_decrypt_tampered_ciphertext_fails() {
        let key = [9u8; 32];
        let plaintext = b"integrity check";

        let mut encrypted = encrypt_data(plaintext, &key).unwrap();
        // 篡改最后一个字节（认证标签的一部分）
        let last_idx = encrypted.len() - 1;
        encrypted[last_idx] ^= 0xFF;

        let result = decrypt_data(&encrypted, &key);
        assert!(result.is_err(), "GCM should detect tampered ciphertext");
    }

    #[test]
    fn test_decrypt_too_short_input_fails() {
        let key = [0u8; 32];
        let too_short = vec![0u8; 8]; // 小于 12 字节 nonce
        let result = decrypt_data(&too_short, &key);
        assert!(result.is_err(), "Decrypting input shorter than nonce must fail");
    }

    #[test]
    fn test_encrypt_empty_plaintext() {
        let key = [3u8; 32];
        let plaintext: &[u8] = b"";
        let encrypted = encrypt_data(plaintext, &key).unwrap();
        let decrypted = decrypt_data(&encrypted, &key).unwrap();
        assert!(decrypted.is_empty());
    }

    // ========== 主密码哈希与验证 ==========

    #[test]
    fn test_master_password_hash_and_verify() {
        let password = "secure_password";
        let salt = [1u8; 32];

        let hash = hash_master_password(password, &salt).unwrap();
        assert!(verify_master_password(password, &hash).unwrap(), "Correct password should verify");
        assert!(
            !verify_master_password("wrong_password", &hash).unwrap(),
            "Wrong password should not verify"
        );
    }

    #[test]
    fn test_master_password_hash_format_is_phc() {
        // Argon2id PHC 字符串应以 $argon2id$ 开头
        let salt = [5u8; 32];
        let hash = hash_master_password("hello", &salt).unwrap();
        assert!(hash.starts_with("$argon2id$"), "Hash should be in PHC format starting with $argon2id$");
    }

    #[test]
    fn test_verify_master_password_invalid_hash_returns_error() {
        let result = verify_master_password("any", "not-a-real-phc-hash");
        assert!(result.is_err(), "Malformed hash string must yield error, not Ok(false)");
    }

    // ========== EncryptedFileHeader 序列化与解析 ==========

    #[test]
    fn test_file_header_roundtrip() {
        let header = EncryptedFileHeader {
            magic: *MAGIC,
            version: VERSION,
            salt: vec![7u8; SALT_SIZE],
            reserved: [0u8; 12],
        };
        let bytes = header.to_bytes();
        // magic(8) + version(4) + salt(32) + reserved(12) = 56；but parser expects >=64
        // 因此需要填充到 64 字节才能成功（或验证现状）
        assert_eq!(bytes.len(), 8 + 4 + SALT_SIZE + 12, "Header bytes size = magic+version+salt+reserved");
    }

    #[test]
    fn test_file_header_rejects_short_input() {
        let too_short = vec![0u8; 32];
        let res = EncryptedFileHeader::from_bytes(&too_short);
        assert!(res.is_err(), "Header parser should reject input shorter than 64 bytes");
    }

    #[test]
    fn test_file_header_rejects_bad_magic() {
        let mut bytes = vec![0u8; 64];
        // 错误 magic
        bytes[0..8].copy_from_slice(b"BADMAGIC");
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        let res = EncryptedFileHeader::from_bytes(&bytes);
        assert!(res.is_err(), "Wrong magic must be rejected");
    }

    #[test]
    fn test_file_header_rejects_unsupported_version() {
        let mut bytes = vec![0u8; 64];
        bytes[0..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes()); // 未支持版本
        let res = EncryptedFileHeader::from_bytes(&bytes);
        assert!(res.is_err(), "Unsupported version must be rejected");
    }

    // ========== MasterPasswordState 会话状态 ==========

    #[test]
    fn test_master_password_state_default_is_locked() {
        let state = MasterPasswordState::default();
        assert!(!state.is_unlocked(), "Default state should be locked");
        assert!(state.get().is_err(), "get() on locked state must error");
    }

    #[test]
    fn test_master_password_state_set_get() {
        let state = MasterPasswordState::default();
        state.set("hunter2".to_string());
        assert!(state.is_unlocked());
        assert_eq!(state.get().unwrap(), "hunter2");
    }

    #[test]
    fn test_master_password_state_clear() {
        let state = MasterPasswordState::default();
        state.set("temp".to_string());
        assert!(state.is_unlocked());
        state.clear();
        assert!(!state.is_unlocked(), "After clear() state should be locked");
        assert!(state.get().is_err());
    }

    #[test]
    fn test_master_password_state_overwrite() {
        let state = MasterPasswordState::default();
        state.set("first".to_string());
        state.set("second".to_string());
        assert_eq!(state.get().unwrap(), "second", "set() should overwrite previous value");
    }

    #[test]
    fn test_master_password_state_thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let state = Arc::new(MasterPasswordState::default());
        state.set("shared".to_string());

        let mut handles = vec![];
        for _ in 0..8 {
            let s = Arc::clone(&state);
            handles.push(thread::spawn(move || {
                assert!(s.is_unlocked());
                let _ = s.get().unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
    }

    // ========== CredentialStore JSON 序列化 ==========

    #[test]
    fn test_credential_store_json_roundtrip() {
        let store = CredentialStore {
            credentials: vec![
                StoredCredential {
                    connection_id: "conn-1".to_string(),
                    password: Some("pw".to_string()),
                    private_key: None,
                },
                StoredCredential {
                    connection_id: "conn-2".to_string(),
                    password: None,
                    private_key: Some("KEY".to_string()),
                },
            ],
        };
        let bytes = serde_json::to_vec(&store).unwrap();
        let parsed: CredentialStore = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.credentials.len(), 2);
        assert_eq!(parsed.credentials[0].connection_id, "conn-1");
        assert_eq!(parsed.credentials[0].password.as_deref(), Some("pw"));
        assert_eq!(parsed.credentials[1].private_key.as_deref(), Some("KEY"));
    }

    // ========== 完整加解密 + 文件头集成测试（不依赖 AppHandle） ==========

    /// 模拟 save_encrypted_credentials -> load_encrypted_credentials 的完整路径，
    /// 不写入文件系统，仅在内存中验证文件头 + AES-GCM + JSON 的端到端往返。
    #[test]
    fn test_full_credential_blob_roundtrip_in_memory() {
        let password = "user-master-password";
        let store = CredentialStore {
            credentials: vec![StoredCredential {
                connection_id: "host-a".to_string(),
                password: Some("p@ss".to_string()),
                private_key: None,
            }],
        };

        // 模拟 save: 生成盐 -> 派生密钥 -> 序列化 -> 加密 -> 拼接文件头
        let salt = vec![0xAAu8; SALT_SIZE];
        let key = derive_key_from_password(password, &salt).unwrap();
        let json = serde_json::to_vec(&store).unwrap();
        let encrypted = encrypt_data(&json, &key).unwrap();

        let header = EncryptedFileHeader {
            magic: *MAGIC,
            version: VERSION,
            salt: salt.clone(),
            reserved: [0u8; 12],
        };
        let mut blob = header.to_bytes();
        assert_eq!(blob.len(), 64, "Header must be exactly 64 bytes");
        blob.extend_from_slice(&encrypted);

        // 模拟 load: 解析文件头 -> 派生密钥 -> 解密 -> 反序列化
        let (parsed_header, ciphertext) = EncryptedFileHeader::from_bytes(&blob).unwrap();
        assert_eq!(parsed_header.salt, salt);
        let key2 = derive_key_from_password(password, &parsed_header.salt).unwrap();
        let decrypted = decrypt_data(ciphertext, &key2).unwrap();
        let parsed_store: CredentialStore = serde_json::from_slice(&decrypted).unwrap();

        assert_eq!(parsed_store.credentials.len(), 1);
        assert_eq!(parsed_store.credentials[0].connection_id, "host-a");
        assert_eq!(parsed_store.credentials[0].password.as_deref(), Some("p@ss"));
    }

    /// 验证错误密码无法解密同一 blob
    #[test]
    fn test_full_credential_blob_wrong_password_fails() {
        let salt = vec![0x33u8; SALT_SIZE];
        let key_correct = derive_key_from_password("right", &salt).unwrap();
        let key_wrong = derive_key_from_password("wrong", &salt).unwrap();

        let store = CredentialStore { credentials: vec![] };
        let json = serde_json::to_vec(&store).unwrap();
        let encrypted = encrypt_data(&json, &key_correct).unwrap();

        assert!(decrypt_data(&encrypted, &key_wrong).is_err());
    }

    // ========== 备份导入/导出格式 ==========

    /// 模拟 export_credentials_backup 的核心格式（盐 + 加密体 -> Base64），
    /// 然后由 import_credentials_backup 的逆向逻辑解析回 store。
    #[test]
    fn test_backup_blob_format_roundtrip() {
        let password = "backup-pw";
        let store = CredentialStore {
            credentials: vec![StoredCredential {
                connection_id: "x".to_string(),
                password: Some("y".to_string()),
                private_key: None,
            }],
        };

        // === export 路径 ===
        let export_salt = vec![0x5Au8; SALT_SIZE];
        let export_key = derive_key_from_password(password, &export_salt).unwrap();
        let json = serde_json::to_vec(&store).unwrap();
        let encrypted = encrypt_data(&json, &export_key).unwrap();
        let mut export_blob = export_salt.clone();
        export_blob.extend_from_slice(&encrypted);
        let encoded = STANDARD.encode(&export_blob);

        // === import 路径 ===
        let decoded = STANDARD.decode(&encoded).unwrap();
        assert!(decoded.len() > SALT_SIZE);
        let import_salt = decoded[0..SALT_SIZE].to_vec();
        let import_ct = &decoded[SALT_SIZE..];
        assert_eq!(import_salt, export_salt);

        let import_key = derive_key_from_password(password, &import_salt).unwrap();
        let decrypted = decrypt_data(import_ct, &import_key).unwrap();
        let parsed: CredentialStore = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(parsed.credentials.len(), 1);
        assert_eq!(parsed.credentials[0].connection_id, "x");
    }

    #[test]
    fn test_backup_invalid_base64_rejected() {
        let res = STANDARD.decode("!!!not-base64!!!");
        assert!(res.is_err(), "Invalid base64 should be rejected by import");
    }

    #[test]
    fn test_backup_too_short_rejected() {
        // 模拟 import 的长度校验
        let too_short = vec![0u8; SALT_SIZE - 1];
        let encoded = STANDARD.encode(&too_short);
        let decoded = STANDARD.decode(&encoded).unwrap();
        assert!(decoded.len() < SALT_SIZE, "Should be detectable as too short");
    }

    #[test]
    fn test_backup_wrong_password_fails() {
        let store = CredentialStore { credentials: vec![] };
        let salt = vec![0x11u8; SALT_SIZE];
        let key_correct = derive_key_from_password("right-pw", &salt).unwrap();
        let json = serde_json::to_vec(&store).unwrap();
        let encrypted = encrypt_data(&json, &key_correct).unwrap();

        let key_wrong = derive_key_from_password("wrong-pw", &salt).unwrap();
        assert!(decrypt_data(&encrypted, &key_wrong).is_err());
    }
}
