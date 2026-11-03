// wirefox-panel is the web panel for a WireFox deployment: it runs next to
// the WireFox server (the core), manages its config and service, and keeps a
// control stream to every exit-node.
package main

import (
	"context"
	"crypto/tls"
	"errors"
	"flag"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/signal"
	"path/filepath"
	"syscall"
	"time"

	"github.com/ParsaKSH/WireFox/panel/internal/agent"
	"github.com/ParsaKSH/WireFox/panel/internal/api"
	"github.com/ParsaKSH/WireFox/panel/internal/core"
	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
	"github.com/ParsaKSH/WireFox/panel/internal/web"
)

const (
	passwordEnv = "WIREFOX_PANEL_PASSWORD"
	// How long open web requests get to finish on shutdown.
	shutdownTimeout = 5 * time.Second
)

// Set at build time: go build -ldflags "-X main.version=v1.2.3".
var version = "dev"

type options struct {
	addr           string
	trustedProxies string
	grpcAddr       string
	dataDir        string
	coreMode       string
	coreConfig     string
	coreBinary     string
	coreUnit       string
	coreData       string
	adminUser      string
	resetPassword  bool
	tls            bool
	tlsCert        string
	tlsKey         string
}

func main() {
	var o options
	flag.StringVar(&o.addr, "addr", "127.0.0.1:8080", "listen address of the web UI")
	flag.StringVar(&o.trustedProxies, "trusted-proxies", "",
		"comma-separated CIDRs of reverse proxies whose X-Forwarded-For/-Host/-Proto headers are believed")
	flag.StringVar(&o.grpcAddr, "grpc-addr", ":50051", "listen address for exit-node agents")
	flag.StringVar(&o.dataDir, "data", "/var/lib/wirefox-panel", "panel data directory")
	flag.StringVar(&o.coreMode, "core-mode", "systemd", "how the core runs: systemd (managed by the panel) or external")
	flag.StringVar(&o.coreConfig, "core-config", "/etc/wirefox/server.toml", "the core's config file")
	flag.StringVar(&o.coreBinary, "core-bin", "/usr/local/bin/wirefox-server", "the core's binary")
	flag.StringVar(&o.coreUnit, "core-unit", "wirefox-server", "systemd unit name of the core")
	flag.StringVar(&o.coreData, "core-data", "/var/lib/wirefox", "data directory for a newly created core config")
	flag.StringVar(&o.adminUser, "admin-user", "admin", "panel admin user name")
	flag.BoolVar(&o.resetPassword, "reset-password", false,
		"set a new admin password (from $"+passwordEnv+" or random), print it and exit")
	flag.BoolVar(&o.tls, "tls", false, "serve the web UI over HTTPS with the panel's own certificate")
	flag.StringVar(&o.tlsCert, "tls-cert", "", "serve the web UI over HTTPS with this certificate (PEM)")
	flag.StringVar(&o.tlsKey, "tls-key", "", "key for -tls-cert (PEM)")
	showVersion := flag.Bool("version", false, "print the version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Println("wirefox-panel", version)
		return
	}

	log := slog.New(slog.NewTextHandler(os.Stderr, nil))
	slog.SetDefault(log)
	if err := run(o, log); err != nil {
		log.Error("fatal", "err", err)
		os.Exit(1)
	}
}

func run(o options, log *slog.Logger) error {
	if o.coreMode != "systemd" && o.coreMode != "external" {
		return fmt.Errorf("-core-mode must be systemd or external, not %q", o.coreMode)
	}
	if (o.tlsCert == "") != (o.tlsKey == "") {
		return errors.New("-tls-cert and -tls-key go together")
	}
	proxies, err := api.ParseProxies(o.trustedProxies)
	if err != nil {
		return fmt.Errorf("-trusted-proxies: %w", err)
	}
	if err := os.MkdirAll(o.dataDir, 0o700); err != nil {
		return err
	}
	st, err := store.Open(filepath.Join(o.dataDir, "panel.db"))
	if err != nil {
		return err
	}
	defer st.Close()

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	if o.resetPassword {
		return resetPassword(ctx, st, o.adminUser)
	}
	if err := ensureAdmin(ctx, st, o.adminUser); err != nil {
		return err
	}

	cert, err := keys.LoadOrCreateCert(filepath.Join(o.dataDir, "panel.crt"), filepath.Join(o.dataDir, "panel.key"))
	if err != nil {
		return err
	}
	log.Info("panel certificate", "fingerprint", cert.Fingerprint)

	coreCfg, apiKey, keyAdded, err := core.Prepare(ctx, st, o.coreConfig, o.coreData)
	if err != nil {
		return fmt.Errorf("preparing the core config: %w", err)
	}
	var manager core.Manager = core.External{}
	if o.coreMode == "systemd" {
		manager = &core.Systemd{
			Unit:       o.coreUnit,
			UnitDir:    "/etc/systemd/system",
			Binary:     o.coreBinary,
			ConfigPath: o.coreConfig,
			DataDir:    coreCfg.DataPath(o.coreConfig),
			Runner:     core.ExecRunner{},
		}
	} else if keyAdded {
		log.Warn("added the panel's API key to the core config; restart the core so it accepts the key",
			"config", o.coreConfig)
	}
	// The panel stays useful while the core can't run, so this isn't fatal.
	if err := manager.Setup(ctx, keyAdded); err != nil {
		log.Error("setting up the core service", "err", err)
	}
	client, err := core.NewClient(coreCfg, o.coreConfig, apiKey)
	if err != nil {
		return err
	}
	tunnel, err := core.NewRunningTunnel(ctx, st, o.coreConfig)
	if err != nil {
		return err
	}

	agents := agent.NewServer(agent.Options{
		Store:    st,
		Endpoint: endpoint(st, tunnel),
		Logger:   log.With("component", "agent"),
	})
	sampler := core.NewSampler(client.Status, core.SampleInterval, func(ctx context.Context, status *core.Status) {
		if err := rememberFingerprint(ctx, st, status.TunnelFingerprint); err != nil {
			log.Warn("saving the core fingerprint", "err", err)
		}
		if err := tunnel.Observe(ctx, status, time.Now()); err != nil {
			log.Warn("reading the core's tunnel listener", "err", err)
		}
		if err := agents.SyncEndpoint(ctx); err != nil {
			log.Warn("updating node configs", "err", err)
		}
	})

	_, grpcPort, err := net.SplitHostPort(o.grpcAddr)
	if err != nil {
		return fmt.Errorf("-grpc-addr: %w", err)
	}
	tlsConfig, err := uiTLS(o, cert)
	if err != nil {
		return err
	}
	if tlsConfig == nil && !loopback(o.addr) {
		log.Warn("the web UI is served over plain HTTP on a public address: passwords, sessions and node keys "+
			"can be read on the way; use -tls, a TLS reverse proxy or an SSH tunnel", "addr", o.addr)
	}

	mux := http.NewServeMux()
	mux.Handle("/api/", api.New(api.Options{
		Store:          st,
		Core:           client,
		Manager:        manager,
		Agents:         agents,
		Sampler:        sampler,
		Tunnel:         tunnel,
		Cert:           cert,
		CoreConfigPath: o.coreConfig,
		CoreBinary:     o.coreBinary,
		GRPCPort:       grpcPort,
		TrustedProxies: proxies,
		Logger:         log.With("component", "api"),
	}))
	mux.Handle("/", web.Handler())
	httpServer := &http.Server{
		Addr:              o.addr,
		Handler:           api.SecurityHeaders(mux),
		TLSConfig:         tlsConfig,
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
		ErrorLog:          slog.NewLogLogger(log.Handler(), slog.LevelWarn),
	}
	grpcServer := agents.GRPCServer(cert)

	httpListener, err := net.Listen("tcp", o.addr)
	if err != nil {
		return err
	}
	grpcListener, err := net.Listen("tcp", o.grpcAddr)
	if err != nil {
		httpListener.Close()
		return err
	}

	// Catches changes made while the panel wasn't running.
	if err := agents.SyncEndpoint(ctx); err != nil {
		log.Warn("updating node configs", "err", err)
	}
	samplerDone := make(chan struct{})
	go func() {
		sampler.Run(ctx)
		close(samplerDone)
	}()
	errs := make(chan error, 2)
	go func() {
		log.Info("agent listener", "addr", grpcListener.Addr().String())
		errs <- grpcServer.Serve(grpcListener)
	}()
	go func() {
		log.Info("web UI", "addr", httpListener.Addr().String(), "https", tlsConfig != nil)
		if tlsConfig != nil {
			errs <- httpServer.ServeTLS(httpListener, "", "")
		} else {
			errs <- httpServer.Serve(httpListener)
		}
	}()

	select {
	case <-ctx.Done():
		log.Info("shutting down")
	case err = <-errs:
	}
	stop()
	shutdownCtx, cancel := context.WithTimeout(context.Background(), shutdownTimeout)
	defer cancel()
	if err := httpServer.Shutdown(shutdownCtx); err != nil {
		log.Warn("stopping the web UI", "err", err)
	}
	// Agent streams never end on their own; nodes reconnect after a restart.
	// Stop waits for their handlers, which record the nodes' last state,
	// so the database is closed only after that.
	grpcServer.Stop()
	<-samplerDone
	return err
}

// uiTLS is the TLS config of the web UI, nil for plain HTTP.
func uiTLS(o options, panelCert *keys.Cert) (*tls.Config, error) {
	cert := panelCert
	switch {
	case o.tlsCert != "":
		var err error
		if cert, err = keys.LoadCert(o.tlsCert, o.tlsKey); err != nil {
			return nil, err
		}
	case !o.tls:
		return nil, nil
	}
	return &tls.Config{Certificates: []tls.Certificate{cert.TLS}, MinVersion: tls.VersionTLS12}, nil
}

// loopback reports whether addr only listens on a loopback address.
func loopback(addr string) bool {
	host, _, err := net.SplitHostPort(addr)
	if err != nil {
		return false
	}
	if host == "localhost" {
		return true
	}
	ip, err := netip.ParseAddr(host)
	return err == nil && ip.IsLoopback()
}

// endpoint tells nodes where to find the core's tunnel listener: the one the
// running core uses, which a changed config file only replaces after a
// restart.
func endpoint(st *store.Store, tunnel *core.RunningTunnel) func(context.Context) (agent.Endpoint, error) {
	return func(ctx context.Context) (agent.Endpoint, error) {
		publicAddr, err := st.Setting(ctx, store.SettingAgentPublicAddr)
		if err != nil {
			return agent.Endpoint{}, err
		}
		fingerprint, err := st.Setting(ctx, store.SettingCoreFingerprint)
		if err != nil {
			return agent.Endpoint{}, err
		}
		return agent.Endpoint{Server: tunnel.Section().Addr(publicAddr), Fingerprint: fingerprint}, nil
	}
}

// rememberFingerprint caches the core's certificate fingerprint so configs
// can be pushed to nodes while the core API is down.
func rememberFingerprint(ctx context.Context, st *store.Store, fingerprint string) error {
	if fingerprint == "" {
		return nil
	}
	known, err := st.Setting(ctx, store.SettingCoreFingerprint)
	if err != nil || known == fingerprint {
		return err
	}
	return st.SetSetting(ctx, store.SettingCoreFingerprint, fingerprint)
}

func ensureAdmin(ctx context.Context, st *store.Store, username string) error {
	exists, err := st.HasAdmin(ctx)
	if err != nil || exists {
		return err
	}
	password, generated := initialPassword()
	if err := setPassword(ctx, st, username, password); err != nil {
		return err
	}
	if generated {
		fmt.Fprintf(os.Stderr, "Created the panel admin %q with password: %s\n", username, password)
	} else {
		fmt.Fprintf(os.Stderr, "Created the panel admin %q with the password from $%s\n", username, passwordEnv)
	}
	return nil
}

func resetPassword(ctx context.Context, st *store.Store, username string) error {
	password, generated := initialPassword()
	if err := setPassword(ctx, st, username, password); err != nil {
		return err
	}
	if err := st.DeleteUserSessions(ctx, username, ""); err != nil {
		return err
	}
	if generated {
		fmt.Fprintf(os.Stderr, "New password for the panel admin %q: %s\n", username, password)
	} else {
		fmt.Fprintf(os.Stderr, "Set the password of the panel admin %q from $%s\n", username, passwordEnv)
	}
	return nil
}

// initialPassword comes from the environment, or is random (generated=true).
func initialPassword() (password string, generated bool) {
	if p := os.Getenv(passwordEnv); p != "" {
		return p, false
	}
	return keys.RandomHex(12), true
}

func setPassword(ctx context.Context, st *store.Store, username, password string) error {
	if err := api.CheckPassword(password); err != nil {
		return err
	}
	hash, err := api.HashPassword(password)
	if err != nil {
		return err
	}
	return st.SetAdmin(ctx, username, hash)
}
