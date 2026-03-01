import { useState, FormEvent } from "react";
import "./ConnectDialog.css";

export interface SshConfig {
  host: string;
  port: number;
  user: string;
  password?: string;
  privateKey?: string;
}

interface ConnectDialogProps {
  onConnect: (config: SshConfig) => void;
  onCancel: () => void;
}

export function ConnectDialog({ onConnect, onCancel }: ConnectDialogProps) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("22");
  const [user, setUser] = useState("");
  const [password, setPassword] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [authType, setAuthType] = useState<"password" | "key">("password");

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    if (!host || !user) return;

    onConnect({
      host,
      port: parseInt(port, 10) || 22,
      user,
      password: authType === "password" ? password : undefined,
      privateKey: authType === "key" ? privateKey : undefined,
    });
  };

  return (
    <div className="dialog-overlay">
      <div className="dialog-content">
        <h2 className="dialog-title">Connect to SSH</h2>
        <form onSubmit={handleSubmit}>
          <div className="form-group">
            <label>Host:</label>
            <input
              type="text"
              value={host}
              onChange={(e) => setHost(e.target.value)}
              placeholder="e.g. 192.168.1.100"
              autoFocus
              required
            />
          </div>
          <div className="form-group">
            <label>Port:</label>
            <input
              type="number"
              value={port}
              onChange={(e) => setPort(e.target.value)}
              required
            />
          </div>
          <div className="form-group">
            <label>User:</label>
            <input
              type="text"
              value={user}
              onChange={(e) => setUser(e.target.value)}
              required
            />
          </div>
          <div className="form-group auth-type-group">
            <label>Auth Type:</label>
            <select
              value={authType}
              onChange={(e) => setAuthType(e.target.value as "password" | "key")}
            >
              <option value="password">Password</option>
              <option value="key">Private Key</option>
            </select>
          </div>
          
          {authType === "password" ? (
            <div className="form-group">
              <label>Password:</label>
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
            </div>
          ) : (
            <div className="form-group">
              <label>Private Key (PEM):</label>
              <textarea
                value={privateKey}
                onChange={(e) => setPrivateKey(e.target.value)}
                placeholder="-----BEGIN RSA PRIVATE KEY-----..."
                rows={4}
              />
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="Key Passphrase (optional)"
                style={{ marginTop: "8px" }}
              />
            </div>
          )}

          <div className="dialog-actions">
            <button type="button" className="btn-cancel" onClick={onCancel}>
              Cancel
            </button>
            <button type="submit" className="btn-connect" disabled={!host || !user}>
              Connect
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
