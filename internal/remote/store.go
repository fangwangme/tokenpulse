// Package remote runs the phone server and its local socket client.
package remote

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"sync"
	"time"
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
		if json.Unmarshal(content, &state) == nil && state.Token != "" {
			return state, nil
		}
	}
	if !errors.Is(err, os.ErrNotExist) && err != nil {
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
	if err := os.MkdirAll(filepath.Dir(s.Path), 0o755); err != nil {
		return err
	}
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return err
	}
	return os.WriteFile(s.Path, data, 0o600)
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
	state.Sessions = append(state.Sessions, Session{Hash: hashToken(token), CreatedAt: time.Now().UTC()})
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
	for _, session := range state.Sessions {
		if session.Hash == hash {
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
