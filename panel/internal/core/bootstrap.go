package core

import (
	"context"
	"errors"
	"io/fs"
	"slices"

	"github.com/ParsaKSH/WireFox/panel/internal/keys"
	"github.com/ParsaKSH/WireFox/panel/internal/store"
)

// Prepare makes sure the core config at configPath exists and accepts the
// panel's API key, whose plaintext lives only in the panel database. A missing
// config is created with defaults; an existing one (adopting an install the
// panel didn't create) gets the panel's key hash appended and is otherwise
// left as it is. keyAdded reports that a running core must restart to accept
// the key.
func Prepare(ctx context.Context, st *store.Store, configPath, dataDir string) (cfg *ServerConfig, apiKey string, keyAdded bool, err error) {
	apiKey, err = st.Setting(ctx, store.SettingCoreAPIKey)
	if err != nil {
		return nil, "", false, err
	}
	if apiKey == "" {
		apiKey = keys.RandomHex(32)
		// Saved first: a key in the config that nobody knows is useless.
		if err := st.SetSetting(ctx, store.SettingCoreAPIKey, apiKey); err != nil {
			return nil, "", false, err
		}
	}
	hash := keys.Hash(apiKey)

	cfg, err = LoadConfig(configPath)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		cfg = DefaultConfig(dataDir)
		cfg.API.APIKeyHashes = []string{hash}
		return cfg, apiKey, false, cfg.Save(configPath)
	case err != nil:
		return nil, "", false, err
	case slices.Contains(cfg.API.APIKeyHashes, hash):
		return cfg, apiKey, false, nil
	}
	cfg.API.APIKeyHashes = append(cfg.API.APIKeyHashes, hash)
	return cfg, apiKey, true, cfg.Save(configPath)
}
