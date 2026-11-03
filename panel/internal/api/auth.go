package api

import (
	"errors"
	"net/http"
	"sync"
	"time"

	"golang.org/x/crypto/bcrypt"

	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

const (
	SessionCookie = "wirefox_session"
	sessionTTL    = 7 * 24 * time.Hour
	// bcrypt ignores everything past 72 bytes.
	maxPasswordLen = 72
	minPasswordLen = 8
)

// Compared against when the username is unknown, so a login takes as long
// whether or not the user exists.
var dummyHash, _ = bcrypt.GenerateFromPassword([]byte("wirefox-panel"), bcrypt.DefaultCost)

// HashPassword hashes an admin password for storage.
func HashPassword(password string) (string, error) {
	if len(password) > maxPasswordLen {
		return "", errors.New("password must be at most 72 bytes")
	}
	hash, err := bcrypt.GenerateFromPassword([]byte(password), bcrypt.DefaultCost)
	return string(hash), err
}

func (a *API) requireSession(h func(http.ResponseWriter, *http.Request, string)) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		cookie, err := r.Cookie(SessionCookie)
		if err != nil || cookie.Value == "" {
			writeError(w, http.StatusUnauthorized, "not logged in")
			return
		}
		username, err := a.Store.SessionUser(r.Context(), keys.Hash(cookie.Value), time.Now())
		if errors.Is(err, store.ErrNotFound) {
			writeError(w, http.StatusUnauthorized, "session expired")
			return
		}
		if err != nil {
			a.internalError(w, r, err)
			return
		}
		h(w, r, username)
	})
}

func (a *API) login(w http.ResponseWriter, r *http.Request) {
	ip := clientIP(r)
	if a.logins.blocked(ip) {
		writeError(w, http.StatusTooManyRequests, "too many failed logins, try again later")
		return
	}
	var req struct {
		Username string `json:"username"`
		Password string `json:"password"`
	}
	if !decode(w, r, &req) {
		return
	}
	ctx := r.Context()
	hash, err := a.Store.AdminHash(ctx, req.Username)
	if err != nil && !errors.Is(err, store.ErrNotFound) {
		a.internalError(w, r, err)
		return
	}
	if err != nil {
		hash = string(dummyHash)
	}
	if bcrypt.CompareHashAndPassword([]byte(hash), []byte(req.Password)) != nil || err != nil {
		a.logins.fail(ip)
		a.log.Warn("failed panel login", "username", req.Username, "remote", ip)
		writeError(w, http.StatusUnauthorized, "wrong username or password")
		return
	}
	a.logins.reset(ip)

	now := time.Now()
	if err := a.Store.PurgeExpiredSessions(ctx, now); err != nil {
		a.log.Warn("purging sessions", "err", err)
	}
	token := keys.RandomHex(32)
	if err := a.Store.CreateSession(ctx, keys.Hash(token), req.Username, now.Add(sessionTTL)); err != nil {
		a.internalError(w, r, err)
		return
	}
	http.SetCookie(w, a.sessionCookie(token, int(sessionTTL/time.Second)))
	writeJSON(w, http.StatusOK, map[string]string{"username": req.Username})
}

func (a *API) sessionCookie(value string, maxAge int) *http.Cookie {
	return &http.Cookie{
		Name:     SessionCookie,
		Value:    value,
		Path:     "/",
		MaxAge:   maxAge,
		HttpOnly: true,
		Secure:   a.SecureCookies,
		SameSite: http.SameSiteStrictMode,
	}
}

func (a *API) logout(w http.ResponseWriter, r *http.Request, _ string) {
	cookie, _ := r.Cookie(SessionCookie)
	if err := a.Store.DeleteSession(r.Context(), keys.Hash(cookie.Value)); err != nil {
		a.internalError(w, r, err)
		return
	}
	http.SetCookie(w, a.sessionCookie("", -1))
	writeOK(w)
}

func (a *API) me(w http.ResponseWriter, _ *http.Request, username string) {
	writeJSON(w, http.StatusOK, map[string]string{"username": username})
}

func (a *API) changePassword(w http.ResponseWriter, r *http.Request, username string) {
	var req struct {
		Current string `json:"current"`
		New     string `json:"new"`
	}
	if !decode(w, r, &req) {
		return
	}
	if len(req.New) < minPasswordLen || len(req.New) > maxPasswordLen {
		writeError(w, http.StatusBadRequest, "the new password must be 8 to 72 characters")
		return
	}
	ctx := r.Context()
	hash, err := a.Store.AdminHash(ctx, username)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	if bcrypt.CompareHashAndPassword([]byte(hash), []byte(req.Current)) != nil {
		writeError(w, http.StatusBadRequest, "the current password is wrong")
		return
	}
	newHash, err := HashPassword(req.New)
	if err != nil {
		a.internalError(w, r, err)
		return
	}
	if err := a.Store.SetAdmin(ctx, username, newHash); err != nil {
		a.internalError(w, r, err)
		return
	}
	// Log out everywhere else.
	cookie, _ := r.Cookie(SessionCookie)
	if err := a.Store.DeleteUserSessions(ctx, username, keys.Hash(cookie.Value)); err != nil {
		a.internalError(w, r, err)
		return
	}
	a.event(ctx, store.Info, "admin password changed")
	writeOK(w)
}

// failureLimiter blocks a client after max failures within window.
type failureLimiter struct {
	max    int
	window time.Duration

	mu       sync.Mutex
	failures map[string][]time.Time
}

func newFailureLimiter(max int, window time.Duration) *failureLimiter {
	return &failureLimiter{max: max, window: window, failures: map[string][]time.Time{}}
}

// recent drops failures older than the window; call with mu held.
func (l *failureLimiter) recent(key string, now time.Time) []time.Time {
	times := l.failures[key]
	i := 0
	for i < len(times) && now.Sub(times[i]) >= l.window {
		i++
	}
	times = times[i:]
	if len(times) == 0 {
		delete(l.failures, key)
	} else {
		l.failures[key] = times
	}
	return times
}

func (l *failureLimiter) blocked(key string) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	return len(l.recent(key, time.Now())) >= l.max
}

func (l *failureLimiter) fail(key string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	now := time.Now()
	l.failures[key] = append(l.recent(key, now), now)
	// Keep the map from growing without bound under a spray of addresses.
	if len(l.failures) > 10000 {
		for k := range l.failures {
			l.recent(k, now)
		}
	}
}

func (l *failureLimiter) reset(key string) {
	l.mu.Lock()
	defer l.mu.Unlock()
	delete(l.failures, key)
}
