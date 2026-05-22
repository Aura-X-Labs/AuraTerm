<template>
  <div v-if="isOpen" class="dialog-overlay" @click.self="handleCancel">
    <div class="dialog-container">
      <div class="dialog-header">
        <h2>{{ mode === 'setup' ? '设置主密码' : '输入主密码' }}</h2>
        <button class="close-btn" @click="handleCancel">×</button>
      </div>

      <div class="dialog-body">
        <template v-if="mode === 'setup'">
          <p class="description">请设置一个强密码来保护您的凭据。此密码将在应用每次启动时需要输入。</p>

          <div class="form-group">
            <label for="password">主密码</label>
            <input
              id="password"
              v-model="password"
              type="password"
              placeholder="输入主密码"
              @keyup.enter="handleSetup"
            />
            <div v-if="password" class="password-strength">
              <div class="strength-bar">
                <div :class="['strength-fill', `strength-${passwordStrength}`]"></div>
              </div>
              <span class="strength-text">{{ passwordStrengthText }}</span>
            </div>
          </div>

          <div class="form-group">
            <label for="passwordConfirm">确认密码</label>
            <input
              id="passwordConfirm"
              v-model="passwordConfirm"
              type="password"
              placeholder="确认密码"
              @keyup.enter="handleSetup"
            />
            <div v-if="passwordConfirm && password !== passwordConfirm" class="error-msg">
              密码不匹配
            </div>
          </div>

          <div class="info-box">
            <strong>提示：</strong>
            <ul>
              <li>密码长度至少 8 字符</li>
              <li>建议混合使用大小写字母、数字和符号</li>
              <li>请牢记此密码，无法恢复</li>
            </ul>
          </div>
        </template>

        <template v-if="mode === 'unlock'">
          <p class="description">请输入主密码以访问保存的连接信息。</p>

          <div class="form-group">
            <label for="unlockPassword">主密码</label>
            <input
              id="unlockPassword"
              v-model="password"
              type="password"
              placeholder="输入主密码"
              @keyup.enter="handleUnlock"
            />
            <div v-if="errorMessage" class="error-msg">
              {{ errorMessage }}
            </div>
          </div>
        </template>
      </div>

      <div class="dialog-footer">
        <button v-if="mode === 'setup'" class="btn btn-secondary" @click="handleCancel">
          取消
        </button>
        <button v-if="mode === 'unlock'" class="btn btn-secondary" @click="handleCancel">
          退出应用
        </button>
        <button
          :class="['btn', 'btn-primary', { disabled: !isFormValid }]"
          :disabled="!isFormValid"
          @click="mode === 'setup' ? handleSetup() : handleUnlock()"
        >
          {{ mode === 'setup' ? '保存' : '解锁' }}
        </button>
      </div>

      <div v-if="isLoading" class="loading-overlay">
        <div class="spinner"></div>
        <p>处理中...</p>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'

type Mode = 'setup' | 'unlock'

interface Props {
  isOpen: boolean
  mode: Mode
}

interface Emits {
  (e: 'success'): void
  (e: 'unlocked'): void
  (e: 'cancel'): void
}

defineProps<Props>()
const emit = defineEmits<Emits>()

const password = ref('')
const passwordConfirm = ref('')
const errorMessage = ref('')
const isLoading = ref(false)

const passwordStrength = computed(() => {
  const pwd = password.value
  if (!pwd) return 'weak'

  let strength = 0
  if (pwd.length >= 8) strength++
  if (pwd.length >= 12) strength++
  if (/[a-z]/.test(pwd) && /[A-Z]/.test(pwd)) strength++
  if (/\d/.test(pwd)) strength++
  if (/[^a-zA-Z0-9]/.test(pwd)) strength++

  if (strength <= 1) return 'weak'
  if (strength <= 3) return 'medium'
  return 'strong'
})

const passwordStrengthText = computed(() => {
  const map = { weak: '弱', medium: '中', strong: '强' }
  return `密码强度：${map[passwordStrength.value]}`
})

const isFormValid = computed(() => {
  if (!password.value) return false
  if (password.value.length < 8) return false

  if (password.value === 'setup') {
    return password.value === passwordConfirm.value && passwordConfirm.value.length >= 8
  }
  return true
})

const handleSetup = async () => {
  if (password.value !== passwordConfirm.value) {
    errorMessage.value = '密码不匹配'
    return
  }

  if (password.value.length < 8) {
    errorMessage.value = '密码长度至少 8 字符'
    return
  }

  isLoading.value = true
  errorMessage.value = ''

  try {
    await invoke('set_master_password', { password: password.value })
    emit('success')
    resetForm()
  } catch (error) {
    errorMessage.value = `设置失败: ${error}`
  } finally {
    isLoading.value = false
  }
}

const handleUnlock = async () => {
  if (!password.value) {
    errorMessage.value = '请输入主密码'
    return
  }

  isLoading.value = true
  errorMessage.value = ''

  try {
    const verified = await invoke<boolean>('verify_master_password', {
      password: password.value,
    })

    if (verified) {
      emit('unlocked')
      resetForm()
    } else {
      errorMessage.value = '密码错误，请重试'
    }
  } catch (error) {
    errorMessage.value = `验证失败: ${error}`
  } finally {
    isLoading.value = false
  }
}

const handleCancel = () => {
  resetForm()
  emit('cancel')
}

const resetForm = () => {
  password.value = ''
  passwordConfirm.value = ''
  errorMessage.value = ''
}

watch(() => passwordConfirm.value, () => {
  if (password.value === passwordConfirm.value) {
    errorMessage.value = ''
  }
})
</script>

<style scoped>
.dialog-overlay {
  position: fixed;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.5);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 1000;
}

.dialog-container {
  position: relative;
  background: var(--vscode-editor-background, #1e1e1e);
  border: 1px solid var(--vscode-panelBorder, #464647);
  border-radius: 6px;
  width: 90%;
  max-width: 400px;
  box-shadow: 0 4px 16px rgba(0, 0, 0, 0.3);
  overflow: hidden;
  display: flex;
  flex-direction: column;
}

.dialog-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 16px;
  border-bottom: 1px solid var(--vscode-panelBorder, #464647);
}

.dialog-header h2 {
  margin: 0;
  font-size: 16px;
  color: var(--vscode-foreground, #e0e0e0);
}

.close-btn {
  background: none;
  border: none;
  font-size: 24px;
  color: var(--vscode-foreground, #e0e0e0);
  cursor: pointer;
  padding: 0;
  width: 24px;
  height: 24px;
  display: flex;
  align-items: center;
  justify-content: center;
}

.close-btn:hover {
  color: var(--vscode-textLink-foreground, #569cd6);
}

.dialog-body {
  padding: 16px;
  flex: 1;
  overflow-y: auto;
  max-height: 400px;
}

.description {
  margin: 0 0 16px 0;
  font-size: 13px;
  color: var(--vscode-descriptionForeground, #a0a0a0);
  line-height: 1.5;
}

.form-group {
  margin-bottom: 12px;
}

.form-group label {
  display: block;
  margin-bottom: 6px;
  font-size: 12px;
  font-weight: 500;
  color: var(--vscode-foreground, #e0e0e0);
}

.form-group input {
  width: 100%;
  padding: 8px;
  border: 1px solid var(--vscode-inputBorder, #464647);
  background: var(--vscode-input-background, #3c3c3c);
  color: var(--vscode-foreground, #e0e0e0);
  border-radius: 4px;
  font-size: 13px;
  box-sizing: border-box;
}

.form-group input:focus {
  outline: none;
  border-color: var(--vscode-focusBorder, #569cd6);
}

.password-strength {
  margin-top: 6px;
  display: flex;
  align-items: center;
  gap: 8px;
}

.strength-bar {
  flex: 1;
  height: 4px;
  background: var(--vscode-inputBorder, #464647);
  border-radius: 2px;
  overflow: hidden;
}

.strength-fill {
  height: 100%;
  width: 100%;
  transition: all 0.3s ease;
}

.strength-fill.strength-weak {
  width: 33%;
  background: #f44747;
}

.strength-fill.strength-medium {
  width: 66%;
  background: #dcdcaa;
}

.strength-fill.strength-strong {
  width: 100%;
  background: #608b4e;
}

.strength-text {
  font-size: 11px;
  color: var(--vscode-descriptionForeground, #a0a0a0);
}

.info-box {
  margin-top: 12px;
  padding: 8px 12px;
  background: rgba(88, 166, 255, 0.1);
  border-left: 3px solid #58a6ff;
  border-radius: 2px;
  font-size: 12px;
  color: var(--vscode-descriptionForeground, #a0a0a0);
}

.info-box strong {
  display: block;
  color: var(--vscode-foreground, #e0e0e0);
  margin-bottom: 6px;
}

.info-box ul {
  margin: 0;
  padding-left: 16px;
}

.info-box li {
  margin: 4px 0;
}

.error-msg {
  margin-top: 4px;
  font-size: 12px;
  color: #f44747;
}

.dialog-footer {
  display: flex;
  gap: 8px;
  padding: 12px 16px;
  border-top: 1px solid var(--vscode-panelBorder, #464647);
  justify-content: flex-end;
}

.btn {
  padding: 6px 12px;
  border: 1px solid var(--vscode-buttonBorder, transparent);
  border-radius: 4px;
  font-size: 13px;
  cursor: pointer;
  transition: all 0.2s ease;
  font-weight: 500;
}

.btn-primary {
  background: var(--vscode-button-background, #0e639c);
  color: var(--vscode-button-foreground, #ffffff);
}

.btn-primary:hover:not(.disabled) {
  background: var(--vscode-button-hoverBackground, #1177bb);
}

.btn-primary.disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn-secondary {
  background: var(--vscode-button-secondaryBackground, #3e3e42);
  color: var(--vscode-button-secondaryForeground, #cccccc);
}

.btn-secondary:hover {
  background: var(--vscode-button-secondaryHoverBackground, #45494b);
}

.loading-overlay {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  bottom: 0;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  z-index: 10;
}

.spinner {
  width: 32px;
  height: 32px;
  border: 3px solid rgba(255, 255, 255, 0.2);
  border-top-color: #ffffff;
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
  margin-bottom: 12px;
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.loading-overlay p {
  color: #ffffff;
  font-size: 13px;
}
</style>
