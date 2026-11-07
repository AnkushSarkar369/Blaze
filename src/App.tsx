function App() {
  return (
    <main className="app-shell">
      <header className="topbar">
        <div className="brand">
          <div className="brand-mark">B</div>
          <span>blaze</span>
          <span className="version">0.1.0</span>
        </div>
        <div className="topbar-right">
          <span className="status-dot" />
          <span className="status-label">LOCAL WORKSPACE</span>
        </div>
      </header>

      <section className="workspace">
        <aside className="sidebar">
          <div className="section-label">WORKSPACE</div>
          <div className="nav-item active">
            <span className="nav-icon">▧</span>
            <span>New project</span>
          </div>
          <div className="sidebar-bottom">
            <div className="sidebar-rule" />
            <div className="sidebar-note">Your footage stays on your machine.</div>
          </div>
        </aside>

        <section className="main-panel">
          <div className="page-heading">
            <div>
              <div className="eyebrow">VIDEO UTILITY</div>
              <h1>Clean your footage up.</h1>
              <p className="subtitle">A cleaner starting point for your next edit.</p>
            </div>
          </div>

          <div className="import-panel">
            <div className="import-symbol">
              <svg viewBox="0 0 48 48" role="presentation">
                <path d="M24 32V8m0 0-9 9m9-9 9 9" />
                <path d="M9 29v9a2 2 0 0 0 2 2h26a2 2 0 0 0 2-2v-9" />
              </svg>
            </div>
            <h2>Start with a video</h2>
            <p>Importing footage will be available in the next build.</p>
            <div className="format-note">VIDEO WORKSPACE <span>·</span> LOCAL PROCESSING</div>
          </div>

          <footer className="workspace-footer">
            <span>BLAZE MODE</span>
            <span className="footer-separator">/</span>
            <span className="muted">READY WHEN YOU ARE</span>
          </footer>
        </section>
      </section>
    </main>
  );
}

export default App;
