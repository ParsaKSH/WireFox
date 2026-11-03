package api

import (
	"encoding/json"
	"io"
	"net/http"
	"net/url"
	"strings"

	"github.com/ParsaKSH/WireFox/panel/internal/core"
)

// proxy forwards a users or connections request to the same path of the
// core API; the core owns those and its answers already have the right shape.
func (a *API) proxy(w http.ResponseWriter, r *http.Request, _ string) {
	body, ok := readBody(w, r)
	if !ok {
		return
	}
	a.forward(w, r, strings.TrimPrefix(r.URL.EscapedPath(), "/api"), body)
}

// createUser forwards to the core, but refuses names the UI couldn't put in
// a URL afterwards.
func (a *API) createUser(w http.ResponseWriter, r *http.Request, _ string) {
	body, ok := readBody(w, r)
	if !ok {
		return
	}
	var req struct {
		Username string `json:"username"`
	}
	if json.Unmarshal(body, &req) == nil && onlyDots(req.Username) {
		writeError(w, http.StatusBadRequest, "username must not be made of dots only")
		return
	}
	a.forward(w, r, "/users", body)
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
	a.forward(w, r, path, nil)
}

func readBody(w http.ResponseWriter, r *http.Request) ([]byte, bool) {
	if r.ContentLength == 0 {
		return nil, true
	}
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxBodySize))
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid request body")
		return nil, false
	}
	return body, true
}

func (a *API) forward(w http.ResponseWriter, r *http.Request, path string, body []byte) {
	status, data, err := a.Core.Do(r.Context(), r.Method, path, body)
	switch {
	case err != nil:
		a.coreError(w, err)
	case status == http.StatusUnauthorized:
		a.coreError(w, &core.APIError{Status: status})
	default:
		writeRaw(w, status, data)
	}
}
