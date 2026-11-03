package api

import (
	"io"
	"net/http"
	"net/url"
	"strings"
)

// proxy forwards a users or connections request to the same path of the
// core API; the core owns those and its answers already have the right shape.
func (a *API) proxy(w http.ResponseWriter, r *http.Request, _ string) {
	a.forward(w, r, strings.TrimPrefix(r.URL.EscapedPath(), "/api"))
}

func (a *API) listConnections(w http.ResponseWriter, r *http.Request, _ string) {
	query := url.Values{}
	for _, key := range []string{"user", "node"} {
		if v := r.URL.Query().Get(key); v != "" {
			query.Set(key, v)
		}
	}
	path := "/connections"
	if len(query) > 0 {
		path += "?" + query.Encode()
	}
	a.forward(w, r, path)
}

func (a *API) forward(w http.ResponseWriter, r *http.Request, path string) {
	var body []byte
	if r.ContentLength != 0 {
		var err error
		body, err = io.ReadAll(http.MaxBytesReader(w, r.Body, maxBodySize))
		if err != nil {
			writeError(w, http.StatusBadRequest, "invalid request body")
			return
		}
	}
	status, data, err := a.Core.Do(r.Context(), r.Method, path, body)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	if status == http.StatusUnauthorized {
		// The core refused the panel's key, not the user's session.
		writeError(w, http.StatusBadGateway, "the WireFox server rejected the panel's API key")
		return
	}
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
	_, _ = w.Write(data)
}
