# AuraTerm 系统钥匙串替换方案 - 详细计划

## 项目目标

替换 macOS/Windows/Linux 系统钥匙串存储方案，改用 **自管理加密凭据存储**，消除频繁弹出的系统授权对话框。

---

## 核心设计

### 整体架构

```
┌─────────────────────────────────────────────────────────┐
│                   应用启动流程                           │
├─────────────────────────────────────────────────────────┤
│  1. 检查是否已设置主密码 (settings.json)                 │
│  2. 未设置 → 显示"首次设置"对话框                        │
│  3. 已设置 → 显示"输入主密码"对话框                      │
│  4. 验证成功 → 密码保存在内存（会话级），应用正常启动    │
│  5. 验证失败 → 提示重新输入，可选择退出                  │
└─────────────────────────────────────────────────────────┘
```

### 密钥派生与加密

```
用户主密码 "MySecurePass123"
    ↓
[Argon2 KDF] (salt + iterations)
    ↓
256-bit 加密密钥
    ↓
[AES-256-GCM] 加密凭据
    ↓
ciphertext + IV + tag → credentials.enc
```

**参数选择：**
- KDF: Argon2id (memory: 16MB, iterations: 3, parallelism: 1)
- 加密: AES-256-GCM (12-byte nonce, 16-byte tag)
- 盐值: 32 字节，随机生成，存储在凭据文件头

---

## 文件存储结构

### 目录布局

```
~/.config/auraterm/           # Linux/macOS
  ├── settings.json           # 包含 masterPasswordHash + salt
  ├── connections.json        # 元数据（id, name, host等，凭据已移出）
  └── credentials.enc         # 加密凭据文件（新建）

C:\Users\user\AppData\Local\auraterm\   # Windows
  ├── settings.json
  ├── connections.json
  └── credentials.enc
```

### credentials.enc 格式

```
[文件头] 64 bytes
  - magic: "AURAENC\0" (8 bytes)
  - version: 1 (4 bytes)
  - argon2_salt: 32 bytes
  - reserved: 12 bytes

[加密数据]
  - IV: 12 bytes (随机 nonce)
  - ciphertext: [...] (加密后的 JSON)
  - auth_tag: 16 bytes (GCM 认证标签)

加密前的 JSON 格式：
{
  "credentials": [
    {
      "connection_id": "conn-123",
      "password": "encrypted_password_hash",
      "private_key": "encrypted_key_content"
    }
  ]
}
```

### settings.json 中的新字段

```json
{
  "fontFamily": "...",
  "masterPasswordHash": "argon2id_hash_result",
  "masterPasswordSalt": "base64_encoded_salt",
  "credentialsInitialized": true
}
```

---

## 后端实现（Rust）

### 1. 依赖变更

**新增到 `Cargo.toml`：**
```toml
aes-gcm = "0.10"          # AES-256-GCM 加密
sha2 = "0.10"             # SHA-256 哈希
argon2 = "0.5"            # 密钥派生函数
zeroize = "1.6"           # 敏感数据清零
base64 = "0.21"           # Base64 编码
rand = "0.8"              # 随机数生成
```

**移除：**
```toml
keyring = { version = "3.6.2", ... }  # 删除
```

### 2. 新建文件：`src-tauri/src/encryption.rs`

**核心结构体：**

```rust
pub struct CredentialStore {
    // 凭据加密/解密的主要逻辑
}

pub struct MasterPassword {
    hash: String,           // Argon2 哈希结果
    salt: Vec<u8>,          // KDF 盐值
}

pub struct EncryptedCredential {
    connection_id: String,
    encrypted_password: Option<Vec<u8>>,
    encrypted_private_key: Option<Vec<u8>>,
}
```

**关键函数：**

```rust
/// 从用户密码推导加密密钥
pub fn derive_key_from_password(
    password: &str,
    salt: &[u8]
) -> Result<[u8; 32], String>

/// 验证用户输入的密码是否与存储的哈希匹配
pub fn verify_master_password(
    password: &str,
    stored_hash: &str,
    salt: &[u8]
) -> Result<bool, String>

/// AES-256-GCM 加密
pub fn encrypt_credentials(
    plaintext: &str,
    key: &[u8; 32]
) -> Result<Vec<u8>, String>

/// AES-256-GCM 解密
pub fn decrypt_credentials(
    ciphertext: &[u8],
    key: &[u8; 32]
) -> Result<String, String>

/// 设置主密码（首次设置或修改）
pub fn set_master_password(
    app: &AppHandle,
    new_password: &str
) -> Result<(), String>

/// 获取已存储的凭据（需要已验证主密码）
pub fn load_encrypted_credentials(app: &AppHandle) -> Result<CredentialStore, String>

/// 保存凭据到加密文件
pub fn save_encrypted_credentials(
    app: &AppHandle,
    credentials: &CredentialStore
) -> Result<(), String>
```

### 3. 修改文件：`src-tauri/src/connections.rs`

**改动点：**

```rust
// 删除所有 keyring 相关代码
// use keyring::Entry;  // 删除
// const KEYRING_SERVICE: &str = "auraterm";  // 删除
// fn secure_entry(), secure_store(), secure_load() 等  // 删除

// 导入新的加密模块
use crate::encryption::{CredentialStore, load_encrypted_credentials, save_encrypted_credentials};

// 修改函数签名：移除 async keyring 调用
fn hydrate_connection_secrets(connection: &mut SavedConnection, store: &CredentialStore) {
    // 从 CredentialStore 中查找并恢复凭据
    // 不再调用 secure_load()
}

fn persist_connection_secrets(connection: &SavedConnection, store: &mut CredentialStore) -> Result<(), String> {
    // 将凭据存储到 CredentialStore
    // 不再调用 secure_store()
}

#[tauri::command]
pub fn get_connections(app: AppHandle) -> Result<Vec<SavedConnection>, String> {
    // 加载凭据存储，然后 hydrate
    let store = load_encrypted_credentials(&app)?;
    // ... 其余逻辑不变
}

#[tauri::command]
pub fn save_connection(app: AppHandle, connection: SavedConnection) -> Result<String, String> {
    // 加载凭据存储，persist，然后保存
    let mut store = load_encrypted_credentials(&app)?;
    persist_connection_secrets(&connection, &mut store)?;
    save_encrypted_credentials(&app, &store)?;
    // ... 其余逻辑不变
}
```

### 4. 修改文件：`src-tauri/src/main.rs`

**添加新命令注册：**

```rust
#[tauri::command]
pub async fn set_master_password(app: AppHandle, password: String) -> Result<(), String> {
    // 调用 encryption::set_master_password()
}

#[tauri::command]
pub async fn verify_master_password(app: AppHandle, password: String) -> Result<bool, String> {
    // 验证主密码，存储到应用状态中
    // 返回 true/false
}

#[tauri::command]
pub async fn export_connections(
    app: AppHandle,
    password: String
) -> Result<String, String> {
    // 验证密码 → 加载凭据 → 返回加密后的 Base64 字符串
}

#[tauri::command]
pub async fn import_connections(
    app: AppHandle,
    password: String,
    encrypted_data: String
) -> Result<(), String> {
    // 验证密码 → 解密导入的凭据 → 合并到现有凭据 → 保存
}

// 在 invoke_handler! 中注册
invoke_handler![
    set_master_password,
    verify_master_password,
    export_connections,
    import_connections,
    // ... 其他命令
]
```

### 5. 应用状态管理

```rust
pub struct AppState {
    pub master_password_verified: bool,
    pub session_key: Option<[u8; 32]>,  // 会话级密钥缓存
}
```

---

## 前端实现（TypeScript/Vue）

### 1. 新建组件：`src/MasterPasswordDialog.vue`

**功能：**
- 首次设置：输入新密码 + 确认密码
- 后续使用：输入主密码以解锁凭据

```vue
<template>
  <Dialog>
    <template v-if="mode === 'setup'">
      <h2>设置主密码</h2>
      <p>请设置一个强密码来保护您的凭据</p>
      <input type="password" v-model="password" placeholder="输入主密码" />
      <input type="password" v-model="passwordConfirm" placeholder="确认密码" />
      <button @click="handleSetup">保存</button>
    </template>
    <template v-if="mode === 'unlock'">
      <h2>解锁凭据</h2>
      <p>请输入主密码以访问保存的连接信息</p>
      <input type="password" v-model="password" placeholder="输入主密码" />
      <button @click="handleUnlock">解锁</button>
    </template>
  </Dialog>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

type DialogMode = 'setup' | 'unlock'

const mode = ref<DialogMode>('setup')
const password = ref('')
const passwordConfirm = ref('')

const handleSetup = async () => {
  if (password.value !== passwordConfirm.value) {
    alert('密码不匹配')
    return
  }
  try {
    await invoke('set_master_password', { password: password.value })
    emit('success')
  } catch (error) {
    alert(`设置失败: ${error}`)
  }
}

const handleUnlock = async () => {
  try {
    const verified = await invoke<boolean>('verify_master_password', { password: password.value })
    if (verified) {
      emit('unlocked')
    } else {
      alert('密码错误')
    }
  } catch (error) {
    alert(`验证失败: ${error}`)
  }
}
</script>
```

### 2. 修改：`src/SettingsDialog.vue`

**新增"安全"标签页：**
```vue
<template>
  <TabGroup>
    <Tab label="常规"><!-- 现有内容 --></Tab>
    <Tab label="主题"><!-- 现有内容 --></Tab>
    <Tab label="安全">
      <div class="security-settings">
        <h3>主密码管理</h3>
        <button @click="showChangeMasterPassword">修改主密码</button>
        
        <h3>凭据导出/导入</h3>
        <button @click="exportCredentials">导出凭据备份</button>
        <button @click="importCredentials">导入凭据备份</button>
      </div>
    </Tab>
  </TabGroup>
</template>

<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { save } from '@tauri-apps/plugin-dialog'

const exportCredentials = async () => {
  try {
    const password = prompt('请输入主密码以导出凭据:')
    if (!password) return
    
    const encrypted = await invoke<string>('export_connections', { password })
    
    const path = await save({
      defaultPath: 'auraterm-credentials-backup.enc',
      filters: [{ name: 'Encrypted Backup', extensions: ['enc'] }]
    })
    
    if (path) {
      await invoke('write_file', { path, contents: encrypted })
      alert('凭据已导出')
    }
  } catch (error) {
    alert(`导出失败: ${error}`)
  }
}

const importCredentials = async () => {
  try {
    // 选择文件 → 读取内容 → 调用 import_connections
    // 详细实现省略
  } catch (error) {
    alert(`导入失败: ${error}`)
  }
}
</script>
```

### 3. 修改：`src/App.vue`

**应用启动流程：**
```typescript
onMounted(async () => {
  try {
    // 检查是否需要设置主密码
    const settings = await invoke<any>('get_settings')
    
    if (!settings.credentialsInitialized) {
      // 显示首次设置对话框
      showMasterPasswordDialog('setup')
    } else {
      // 显示解锁对话框
      showMasterPasswordDialog('unlock')
    }
  } catch (error) {
    console.error('初始化失败:', error)
  }
})
```

---

## 迁移策略

### 用户首次启动

1. **检测**：系统钥匙串中是否存在旧凭据
2. **提示**：显示迁移对话框
   ```
   已检测到系统钥匙串中的旧凭据
   新版本使用本地加密存储，旧凭据将不可用
   
   推荐：导出旧凭据作为备份，然后手动重新添加新连接
   
   [了解详情] [导出旧凭据] [继续]
   ```
3. **处理**：不自动迁移（让用户自行决定）

### 导出旧凭据（可选功能）

如果用户选择导出，提供一个临时的 CLI 工具或脚本来读取系统钥匙串并保存为明文JSON（供用户备份）。

---

## 技术决策总结

| 选项 | 决策 | 理由 |
|------|------|------|
| KDF | Argon2id | 抗 GPU 破解，内存困难 |
| 加密 | AES-256-GCM | 硬件加速，认证加密 |
| 主密码超时 | 应用会话级 | 简单安全，应用关闭重新输入 |
| 导出格式 | 加密 JSON | 跨平台兼容，便于版本升级 |
| 迁移方式 | 手动 | 避免数据丢失，用户可控 |
| 系统钥匙串 | 完全移除 | 消除弹窗问题 |

---

## 实现步骤顺序

### Phase 1: 后端基础（3-4 步）
1. ✅ 添加 Rust 依赖
2. ✅ 实现 `encryption.rs` 模块
3. ✅ 修改 `connections.rs` 集成加密
4. ✅ 新增 Tauri 命令

### Phase 2: 前端 UI（3 步）
5. ✅ 实现 `MasterPasswordDialog.vue`
6. ✅ 修改 `SettingsDialog.vue` 安全标签页
7. ✅ 修改 `App.vue` 启动流程

### Phase 3: 迁移与测试（3-4 步）
8. ✅ 编写迁移提示 UI
9. ✅ 实现导出/导入功能
10. ✅ 移除 keyring 依赖
11. ✅ 跨平台测试

---

## 关键代码示例

### Rust: 主密码验证

```rust
use argon2::{Argon2, ParamsBuilder, Version, Variant};
use base64::{engine::general_purpose, Engine as _};

pub fn verify_master_password(
    input_password: &str,
    stored_hash: &str,
    salt_b64: &str,
) -> Result<bool, String> {
    let salt = general_purpose::STANDARD
        .decode(salt_b64)
        .map_err(|e| format!("Salt decode failed: {e}"))?;

    let config = Argon2::new(
        Variant::Id,
        Version::V0x13,
        ParamsBuilder::new()
            .m_cost(16 * 1024)  // 16 MB
            .t_cost(3)
            .p_cost(1)
            .build()
            .unwrap(),
    );

    let hash_result = config
        .hash_password(input_password.as_bytes(), &salt)
        .map_err(|e| e.to_string())?
        .to_string();

    Ok(hash_result == stored_hash)
}
```

### TypeScript: 调用加密命令

```typescript
// 保存凭据时
async function saveConnectionWithEncryption(connection: SavedConnection) {
  try {
    // 1. 保存连接元数据
    await invoke('save_connection', { connection })
    
    // 2. 凭据已自动加密（在后端完成）
    console.log('连接已保存并加密')
  } catch (error) {
    console.error('保存失败:', error)
  }
}

// 读取凭据时
async function loadConnectionsWithDecryption() {
  try {
    const connections = await invoke<SavedConnection[]>('get_connections')
    // 凭据已自动解密（在后端完成）
    return connections
  } catch (error) {
    console.error('读取失败:', error)
  }
}
```

---

## 风险和缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 用户忘记主密码 | 无法访问凭据 | 提醒定期备份导出 |
| 密钥文件被盗 | 凭据可能泄露 | 使用强主密码 + Argon2 防破解 |
| 跨平台兼容性 | 迁移问题 | 充分测试，提供导出/导入 |
| 性能影响 | 启动变慢 | Argon2 配置优化，缓存会话密钥 |

---

## 测试清单

- [ ] 首次启动：设置主密码流程
- [ ] 二次启动：输入主密码解锁
- [ ] 错误密码：重试机制
- [ ] 保存连接：凭据加密存储
- [ ] 读取连接：凭据正确解密
- [ ] 修改主密码：旧凭据重新加密
- [ ] 导出凭据：生成加密备份文件
- [ ] 导入凭据：正确合并恢复
- [ ] macOS 测试：所有功能正常
- [ ] Windows 测试：所有功能正常
- [ ] Linux 测试：所有功能正常
- [ ] 密钥文件权限：确保 0600

---

## 后续优化空间

1. **硬件加速**：调用平台的加密库加速（如 OpenSSL）
2. **生物识别**：整合指纹/Face ID 解锁（macOS）
3. **云端同步**：支持加密备份同步（可选）
4. **密码强度检测**：UI 提示密码强度
5. **会话超时**：可配置的会话过期时间
