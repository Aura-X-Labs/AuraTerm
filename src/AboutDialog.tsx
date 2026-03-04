import "./AboutDialog.css";

interface AboutDialogProps {
  onClose: () => void;
}

export function AboutDialog({ onClose }: AboutDialogProps) {
  return (
    <div className="about-overlay" onClick={onClose}>
      <div className="about-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="about-header">
          <h2>About AuraTerm</h2>
          <button className="about-close-btn" onClick={onClose} type="button">
            ×
          </button>
        </div>

        <div className="about-body">
          <div className="about-logo">🖥️</div>
          
          <div className="about-content">
            <h3>AuraTerm</h3>
            <p className="about-version">Version 0.1.0</p>
            
            <p className="about-description">
              A modern terminal application built with Tauri and React, featuring SSH and Telnet connections.
            </p>

            <div className="about-features">
              <h4>Features</h4>
              <ul>
                <li>SSH and Telnet connections</li>
                <li>Multi-tab support</li>
                <li>Bookmark management</li>
                <li>Customizable settings</li>
                <li>Light and dark theme support</li>
              </ul>
            </div>

            <div className="about-info">
              <p><strong>Built with:</strong> Tauri + React + TypeScript</p>
              <p><strong>License:</strong> MIT</p>
            </div>

            <div className="about-links">
              <button
                className="about-link-btn"
                onClick={() => window.open("https://github.com/Aura-X-Labs/AuraTerm", "_blank")}
                type="button"
              >
                GitHub Repository
              </button>
              <button
                className="about-link-btn"
                onClick={() => window.open("https://github.com/Aura-X-Labs/AuraTerm/issues", "_blank")}
                type="button"
              >
                Report Issues
              </button>
            </div>
          </div>
        </div>

        <div className="about-footer">
          <button className="about-ok-btn" onClick={onClose} type="button">
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
