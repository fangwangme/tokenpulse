package remote

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"path/filepath"
	"time"

	"github.com/gorilla/websocket"
)

// LocalClient talks to a running serve over its Unix socket.
type LocalClient struct {
	Socket string
	client *http.Client
}

func NewLocalClient(dataDir string) *LocalClient {
	socket := filepath.Join(dataDir, "serve.sock")
	transport := &http.Transport{
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			dialer := net.Dialer{}
			return dialer.DialContext(ctx, "unix", socket)
		},
	}
	return &LocalClient{Socket: socket, client: &http.Client{Transport: transport, Timeout: 120 * time.Second}}
}

type StateInfo struct {
	Version         string         `json:"version"`
	ScannedAt       int64          `json:"scannedAt"`
	FetchedAt       int64          `json:"fetchedAt"`
	PricesAvailable bool           `json:"pricesAvailable"`
	Agents          []string       `json:"agents"`
	Settings        map[string]any `json:"settings"`
	KeeperEngine    bool           `json:"keeperEngine"`
}

func (c *LocalClient) do(method, path string, body any, out any) (int, error) {
	var reader io.Reader
	if body != nil {
		data, err := json.Marshal(body)
		if err != nil {
			return 0, err
		}
		reader = bytes.NewReader(data)
	}
	request, err := http.NewRequest(method, "http://localhost"+path, reader)
	if err != nil {
		return 0, err
	}
	request.Host = "localhost"
	if body != nil {
		request.Header.Set("Content-Type", "application/json")
	}
	response, err := c.client.Do(request)
	if err != nil {
		return 0, err
	}
	defer response.Body.Close()
	data, err := io.ReadAll(io.LimitReader(response.Body, 16*1024*1024))
	if err != nil {
		return response.StatusCode, err
	}
	if out != nil && len(data) > 0 {
		if err := json.Unmarshal(data, out); err != nil {
			return response.StatusCode, err
		}
	}
	if response.StatusCode >= 400 {
		var payload struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(data, &payload) == nil && payload.Error != "" {
			return response.StatusCode, errors.New(payload.Error)
		}
		return response.StatusCode, errors.New(response.Status)
	}
	return response.StatusCode, nil
}

func (c *LocalClient) Get(path string, out any) error {
	_, err := c.do(http.MethodGet, path, nil, out)
	return err
}

func (c *LocalClient) Post(path string, body any, out any) error {
	_, err := c.do(http.MethodPost, path, body, out)
	return err
}

func (c *LocalClient) Patch(path string, body any, out any) error {
	_, err := c.do(http.MethodPatch, path, body, out)
	return err
}

// State returns the server state when serve answers.
func (c *LocalClient) State() (*StateInfo, error) {
	var state StateInfo
	if err := c.Get("/api/state", &state); err != nil {
		return nil, err
	}
	return &state, nil
}

// WebSocket connects to serve's event stream.
func (c *LocalClient) WebSocket() (*websocket.Conn, error) {
	dialer := websocket.Dialer{
		NetDialContext: func(ctx context.Context, network, addr string) (net.Conn, error) {
			var dial net.Dialer
			return dial.DialContext(ctx, "unix", c.Socket)
		},
		HandshakeTimeout: 10 * time.Second,
	}
	conn, _, err := dialer.Dial("ws://localhost/ws", nil)
	return conn, err
}
