// Package api is the panel's JSON API for the web UI (see panel/API.md).
package api

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"log/slog"
	"mime"
	"net/http"
	"net/url"
	"strconv"
	"strings"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/agent"
	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const (
	maxBodySize = 1 << 20

	// The lines parameter of the log endpoints.
	defaultLogLines = 200
	maxLogLines     = 5000

	// How long calls that only add detail to an answer may take.
	quickTimeout = 3 * time.Second
)

// Options configure the API. Everything except Logger and TrustedProxies is
// required.
type Options struct {
	Store   *store.Store
	Core    *core.Client
	Manager core.Manager
	Agents  *agent.Server
	Sampler *core.Sampler
	Tunnel  *core.RunningTunnel
	Cert    *keys.Cert
	// The core's config file and binary, as shown in GET /api/core.
	CoreConfigPath string
	CoreBinary     string
	// Port of the gRPC listener, used in node keys while agent_public_addr
	// isn't set.
	GRPCPort string
	// Reverse proxies whose X-Forwarded-* headers are believed.
	TrustedProxies Proxies
	Logger         *slog.Logger
}

type API struct {
	Options
	log    *slog.Logger
	logins *failureLimiter
	// Bounds the bcrypt comparisons running at once.
	bcryptSlots chan struct{}
}

// New returns the handler for everything under /api/.
func New(opts Options) http.Handler {
	a := &API{
		Options:     opts,
		log:         opts.Logger,
		logins:      newFailureLimiter(maxLoginFailures, loginFailureWindow),
		bcryptSlots: make(chan struct{}, maxConcurrentLogins),
	}
	if a.log == nil {
		a.log = slog.Default()
	}

	mux := http.NewServeMux()
	mux.HandleFunc("POST /api/login", a.login)

	auth := func(pattern string, h func(http.ResponseWriter, *http.Request, string)) {
		mux.Handle(pattern, a.requireSession(h))
	}
	auth("POST /api/logout", a.logout)
	auth("GET /api/me", a.me)
	auth("POST /api/password", a.changePassword)

	auth("GET /api/overview", a.overview)

	auth("GET /api/nodes", a.listNodes)
	auth("POST /api/nodes", a.createNode)
	auth("GET /api/nodes/{id}", a.getNode)
	auth("PATCH /api/nodes/{id}", a.updateNode)
	auth("DELETE /api/nodes/{id}", a.deleteNode)
	auth("POST /api/nodes/{id}/key", a.newNodeKey)
	auth("POST /api/nodes/{id}/restart", a.restartNode)
	auth("POST /api/nodes/{id}/interfaces", a.nodeInterfaces)
	auth("GET /api/nodes/{id}/logs", a.nodeLogs)
	auth("POST /api/nodes/{id}/reset-traffic", a.resetNodeTraffic)

	auth("GET /api/users", a.proxy)
	auth("POST /api/users", a.createUser)
	auth("PATCH /api/users/{name}", a.proxy)
	auth("DELETE /api/users/{name}", a.proxy)
	auth("POST /api/users/{name}/reset-traffic", a.proxy)

	auth("GET /api/connections", a.listConnections)
	auth("DELETE /api/connections/{id}", a.proxy)

	auth("GET /api/core", a.getCore)
	auth("PUT /api/core/config", a.putCoreConfig)
	auth("POST /api/core/start", a.coreAction(core.Manager.Start, "started"))
	auth("POST /api/core/stop", a.coreAction(core.Manager.Stop, "stopped"))
	auth("POST /api/core/restart", a.coreAction(a.restartCore, "restarted"))
	auth("GET /api/core/logs", a.coreLogs)

	auth("GET /api/settings", a.getSettings)
	auth("PUT /api/settings", a.putSettings)

	auth("GET /api/events", a.events)

	mux.HandleFunc("/api/", func(w http.ResponseWriter, r *http.Request) {
		writeError(w, http.StatusNotFound, "no such endpoint")
	})
	return a.sameOriginJSON(mux)
}

// sameOriginJSON turns away state-changing requests a browser could send from
// another site: a body must be JSON (forms can't send that without CORS), and
// an Origin header, when present, must be our own.
func (a *API) sameOriginJSON(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.Method {
		case http.MethodGet, http.MethodHead, http.MethodOptions:
			next.ServeHTTP(w, r)
			return
		}
		if origin := r.Header.Get("Origin"); origin != "" {
			u, err := url.Parse(origin)
			if err != nil || u.Host != a.TrustedProxies.host(r) {
				writeError(w, http.StatusForbidden, "cross-origin request refused")
				return
			}
		}
		contentType := r.Header.Get("Content-Type")
		if contentType != "" || r.ContentLength != 0 {
			mediaType, _, err := mime.ParseMediaType(contentType)
			if err != nil || mediaType != "application/json" {
				writeError(w, http.StatusUnsupportedMediaType, "request body must be application/json")
				return
			}
		}
		next.ServeHTTP(w, r)
	})
}

// SecurityHeaders sets the headers every panel response carries.
func SecurityHeaders(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		h := w.Header()
		h.Set("Content-Security-Policy",
			"default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'")
		h.Set("X-Frame-Options", "DENY")
		h.Set("X-Content-Type-Options", "nosniff")
		h.Set("Referrer-Policy", "no-referrer")
		next.ServeHTTP(w, r)
	})
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	jsonHeaders(w, status)
	_ = json.NewEncoder(w).Encode(v)
}

// writeRaw answers with a body that already is JSON.
func writeRaw(w http.ResponseWriter, status int, data []byte) {
	jsonHeaders(w, status)
	_, _ = w.Write(data)
}

func jsonHeaders(w http.ResponseWriter, status int) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(status)
}

func writeError(w http.ResponseWriter, status int, msg string) {
	writeJSON(w, status, map[string]string{"error": msg})
}

func writeOK(w http.ResponseWriter) {
	writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
}

// internalError logs err and answers with a generic 500.
func (a *API) internalError(w http.ResponseWriter, r *http.Request, err error) {
	a.log.Error("request failed", "method", r.Method, "path", r.URL.Path, "err", err)
	writeError(w, http.StatusInternalServerError, "internal error")
}

// coreError answers for a failed core API call: the core's own errors keep
// their status, an unreachable core is a 502.
func (a *API) coreError(w http.ResponseWriter, err error) {
	var apiErr *core.APIError
	switch {
	case errors.As(err, &apiErr) && apiErr.Status == http.StatusUnauthorized:
		// Our key, not the user's session: don't log the user out.
		writeError(w, http.StatusBadGateway, "the WireFox server rejected the panel's API key")
	case errors.As(err, &apiErr):
		writeError(w, apiErr.Status, apiErr.Message)
	default:
		writeError(w, http.StatusBadGateway, err.Error())
	}
}

// decode reads a JSON body into v, refusing unknown fields.
func decode(w http.ResponseWriter, r *http.Request, v any) bool {
	dec := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxBodySize))
	dec.DisallowUnknownFields()
	if err := dec.Decode(v); err != nil {
		msg := "invalid request body"
		if !errors.Is(err, io.EOF) {
			msg += ": " + err.Error()
		}
		writeError(w, http.StatusBadRequest, msg)
		return false
	}
	return true
}

// queryInt reads a positive integer query parameter, clamped to max.
func queryInt(r *http.Request, name string, def, max int) int {
	n, err := strconv.Atoi(r.URL.Query().Get(name))
	if err != nil || n <= 0 {
		return def
	}
	return min(n, max)
}

// onlyDots reports whether id is made of dots only. Browsers resolve "." and
// ".." in URL paths, so such a node or user could never be addressed.
func onlyDots(id string) bool {
	return id != "" && strings.Trim(id, ".") == ""
}

// quick bounds calls that only add detail to an answer, so a hanging core
// doesn't hold up the page.
func quick(ctx context.Context) (context.Context, context.CancelFunc) {
	return context.WithTimeout(ctx, quickTimeout)
}
