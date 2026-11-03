// Package web serves the panel's single-page frontend, built into dist and
// embedded in the binary.
package web

import (
	"bytes"
	"embed"
	"io/fs"
	"net/http"
	"path"
	"strings"
	"time"
)

//go:embed all:dist
var dist embed.FS

const notBuilt = `<!doctype html>
<html><head><meta charset="utf-8"><title>wirefox-panel</title></head>
<body>
<h1>wirefox-panel</h1>
<p>This binary was built without the web frontend.</p>
<p>Build it with <code>make build</code> in the <code>panel</code> directory
(needs Node.js and npm), or run <code>npm ci &amp;&amp; npm run build</code> in
<code>panel/frontend</code> and rebuild the panel.</p>
</body></html>
`

// Handler serves the frontend from the embedded dist directory.
func Handler() http.Handler {
	files, err := fs.Sub(dist, "dist")
	if err != nil {
		panic(err)
	}
	return newHandler(files)
}

func newHandler(files fs.FS) http.Handler {
	// Nil when the frontend wasn't built.
	index, _ := fs.ReadFile(files, "index.html")
	fileServer := http.FileServerFS(files)
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet && r.Method != http.MethodHead {
			w.Header().Set("Allow", "GET, HEAD")
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		if index == nil {
			w.Header().Set("Content-Type", "text/html; charset=utf-8")
			_, _ = w.Write([]byte(notBuilt))
			return
		}
		name := strings.TrimPrefix(path.Clean(r.URL.Path), "/")
		if name != "" && name != "index.html" && !strings.HasPrefix(name, ".") {
			if info, err := fs.Stat(files, name); err == nil && !info.IsDir() {
				// Vite puts content hashes in asset names.
				if strings.HasPrefix(name, "assets/") {
					w.Header().Set("Cache-Control", "public, max-age=31536000, immutable")
				}
				fileServer.ServeHTTP(w, r)
				return
			}
		}
		// Client-side routes all get the app shell.
		w.Header().Set("Cache-Control", "no-cache")
		http.ServeContent(w, r, "index.html", time.Time{}, bytes.NewReader(index))
	})
}
