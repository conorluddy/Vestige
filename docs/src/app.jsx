// app.jsx — page composition + mount.

function VestigePage() {
  return (
    <div className="vt-page">
      <Bar />
      <Hero />
      <Thesis />
      <Disclosure />
      <Recall />
      <MCP />
      <Skills />
      <Storage />
      <SchemaSection />
      <Embeddings />
      <Provenance />
      <Browser />
      <Features />
      <CLI />
      <Roadmap />
      <Footer />
    </div>
  );
}

ReactDOM.createRoot(document.getElementById('root')).render(<VestigePage />);
