// Package remote runs the phone server and its local socket client.
package remote

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const (
	sessionTTL  = 400 * 24 * time.Hour
	maxSessions = 20
)

// Pairing state lives in remote.json with mode 0600.
type State struct {
	Token    string    `json:"token"`
	Sessions []Session `json:"sessions"`
}

type Session struct {
	Hash      string    `json:"hash"`
	CreatedAt time.Time `json:"createdAt"`
}

type StateStore struct {
	Path string
	mu   sync.Mutex
}

func NewStateStore(dataDir string) *StateStore {
	return &StateStore{Path: filepath.Join(dataDir, "remote.json")}
}

func randomToken(bytes int) (string, error) {
	buffer := make([]byte, bytes)
	if _, err := rand.Read(buffer); err != nil {
		return "", err
	}
	return hex.EncodeToString(buffer), nil
}

func hashToken(token string) string {
	sum := sha256.Sum256([]byte(token))
	return hex.EncodeToString(sum[:])
}

// Load reads the state, creating the remote token on first use.
func (s *StateStore) Load() (State, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.loadLocked()
}

func (s *StateStore) loadLocked() (State, error) {
	content, err := os.ReadFile(s.Path)
	if err == nil {
		var state State
		if err := json.Unmarshal(content, &state); err != nil {
			return State{}, fmt.Errorf("remote state is corrupt: %w", err)
		}
		if state.Token == "" {
			return State{}, errors.New("remote state has no token")
		}
		return state, nil
	}
	if !errors.Is(err, os.ErrNotExist) {
		return State{}, err
	}
	token, err := randomToken(16)
	if err != nil {
		return State{}, err
	}
	state := State{Token: token}
	if err := s.saveLocked(state); err != nil {
		return State{}, err
	}
	return state, nil
}

func (s *StateStore) saveLocked(state State) error {
	directory := filepath.Dir(s.Path)
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return err
	}
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return err
	}
	temporary, err := os.CreateTemp(directory, ".remote-*.tmp")
	if err != nil {
		return err
	}
	name := temporary.Name()
	defer os.Remove(name)
	if err := temporary.Chmod(0o600); err != nil {
		temporary.Close()
		return err
	}
	if _, err := temporary.Write(data); err != nil {
		temporary.Close()
		return err
	}
	if err := temporary.Sync(); err != nil {
		temporary.Close()
		return err
	}
	if err := temporary.Close(); err != nil {
		return err
	}
	if err := os.Rename(name, s.Path); err != nil {
		return err
	}
	if directoryFile, err := os.Open(directory); err == nil {
		_ = directoryFile.Sync()
		_ = directoryFile.Close()
	}
	return nil
}

// Token returns the remote token.
func (s *StateStore) Token() (string, error) {
	state, err := s.Load()
	return state.Token, err
}

// ResetToken writes a new remote token and deletes every session.
func (s *StateStore) ResetToken() (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	token, err := randomToken(16)
	if err != nil {
		return "", err
	}
	if err := s.saveLocked(State{Token: token}); err != nil {
		return "", err
	}
	return token, nil
}

// CreateSession stores only the session token's hash and returns the token.
func (s *StateStore) CreateSession() (string, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	state, err := s.loadLocked()
	if err != nil {
		return "", err
	}
	token, err := randomToken(32)
	if err != nil {
		return "", err
	}
	now := time.Now().UTC()
	active := make([]Session, 0, len(state.Sessions)+1)
	for _, session := range state.Sessions {
		if now.Sub(session.CreatedAt) < sessionTTL {
			active = append(active, session)
		}
	}
	if len(active) >= maxSessions {
		active = active[len(active)-maxSessions+1:]
	}
	state.Sessions = append(active, Session{Hash: hashToken(token), CreatedAt: now})
	if err := s.saveLocked(state); err != nil {
		return "", err
	}
	return token, nil
}

func (s *StateStore) ValidSession(token string) bool {
	if token == "" {
		return false
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	state, err := s.loadLocked()
	if err != nil {
		return false
	}
	hash := hashToken(token)
	now := time.Now().UTC()
	for _, session := range state.Sessions {
		if session.Hash == hash && now.Sub(session.CreatedAt) < sessionTTL {
			return true
		}
	}
	return false
}

func (s *StateStore) DeleteSessions() error {
	s.mu.Lock()
	defer s.mu.Unlock()
	state, err := s.loadLocked()
	if err != nil {
		return err
	}
	state.Sessions = nil
	return s.saveLocked(state)
}
