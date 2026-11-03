package web

import (
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"testing/fstest"
)

func get(t *testing.T, h http.Handler, method, path string) (*http.Response, string) {
	t.Helper()
	rec := httptest.NewRecorder()
	h.ServeHTTP(rec, httptest.NewRequest(method, path, nil))
	body, _ := io.ReadAll(rec.Result().Body)
	return rec.Result(), string(body)
}

func TestSPA(t *testing.T) {
	h := newHandler(fstest.MapFS{
		"index.html":       {Data: []byte("<html>app</html>")},
		"assets/app-1.js":  {Data: []byte("console.log(1)")},
		"favicon.svg":      {Data: []byte("<svg/>")},
		".gitkeep":         {Data: nil},
		"assets/style.css": {Data: []byte("body{}")},
	})
	for path, want := range map[string]string{
		"/":                "<html>app</html>",
		"/nodes/office-1":  "<html>app</html>",
		"/index.html":      "<html>app</html>",
		"/.gitkeep":        "<html>app</html>",
		"/assets/app-1.js": "console.log(1)",
		"/favicon.svg":     "<svg/>",
	} {
		resp, body := get(t, h, http.MethodGet, path)
		if resp.StatusCode != 200 || body != want {
			t.Errorf("%s: %d %q", path, resp.StatusCode, body)
		}
	}
	if resp, _ := get(t, h, http.MethodGet, "/assets/app-1.js"); !strings.Contains(resp.Header.Get("Cache-Control"), "immutable") {
		t.Errorf("asset caching: %q", resp.Header.Get("Cache-Control"))
	}
	if resp, _ := get(t, h, http.MethodPost, "/"); resp.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("POST: %d", resp.StatusCode)
	}
}

func TestNotBuilt(t *testing.T) {
	h := newHandler(fstest.MapFS{".gitkeep": {Data: nil}})
	resp, body := get(t, h, http.MethodGet, "/anything")
	if resp.StatusCode != 200 || !strings.Contains(body, "without the web frontend") {
		t.Fatalf("%d %q", resp.StatusCode, body)
	}
}
