package remote

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"net/url"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/keeper"
	"github.com/fangwangme/tokenpulse/internal/report"
	rt "github.com/fangwangme/tokenpulse/internal/runtime"
)

// LocalClient talks to a running serve over its Unix socket.
type LocalClient struct {
	Socket string
	client *http.Client
	err    error
}

func NewLocalClient(dataDir string) *LocalClient {
	socket, err := SocketPath(dataDir)
	if err != nil {
		return &LocalClient{err: err}
	}
	transport := &http.Transport{
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			dialer := net.Dialer{}
			return dialer.DialContext(ctx, "unix", socket)
		},
	}
	return &LocalClient{Socket: socket, client: &http.Client{Transport: transport, Timeout: 120 * time.Second}}
}

// WaitForSocket polls until a running serve answers on its Unix socket or
// timeout elapses.
func WaitForSocket(dataDir string, timeout time.Duration) (*LocalClient, error) {
	client := NewLocalClient(dataDir)
	deadline := time.Now().Add(timeout)
	var lastErr error
	for {
		if _, err := client.Status(); err == nil {
			return client, nil
		} else {
			lastErr = err
		}
		if time.Now().After(deadline) {
			return nil, lastErr
		}
		time.Sleep(50 * time.Millisecond)
	}
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
	if c.err != nil {
		return 0, c.err
	}
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

func (c *LocalClient) State() (*StateInfo, error) {
	var state StateInfo
	if err := c.Get("/api/state", &state); err != nil {
		return nil, err
	}
	return &state, nil
}

// StatusInfo is the non-blocking server refresh state.
type StatusInfo = rt.StatusSnapshot

func (c *LocalClient) Status() (*StatusInfo, error) {
	var status StatusInfo
	if err := c.Get("/api/status", &status); err != nil {
		return nil, err
	}
	return &status, nil
}

// Shutdown asks a running local serve to exit gracefully.
func (c *LocalClient) Shutdown() error {
	return c.Post("/api/shutdown", map[string]any{}, nil)
}

func (c *LocalClient) WebSocket() (*websocket.Conn, error) {
	if c.err != nil {
		return nil, c.err
	}
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

// SocketClient adapts LocalClient to the unified runtime.Client interface.
type SocketClient struct {
	Local *LocalClient
}

func NewSocketClient(local *LocalClient) *SocketClient {
	return &SocketClient{Local: local}
}

func (s *SocketClient) Status() (rt.StatusSnapshot, error) {
	st, err := s.Local.Status()
	if err != nil {
		return rt.StatusSnapshot{}, err
	}
	return *st, nil
}

func (s *SocketClient) Config() (config.Config, error) {
	var cfg config.Config
	err := s.Local.Get("/api/config", &cfg)
	return cfg, err
}

func (s *SocketClient) Usage(params report.Params) (report.Report, error) {
	q := url.Values{}
	if params.Preset != "" {
		q.Set("preset", params.Preset)
	}
	if params.From != "" {
		q.Set("from", params.From)
	}
	if params.To != "" {
		q.Set("to", params.To)
	}
	if len(params.Agents) > 0 {
		q.Set("agents", strings.Join(apiAgentNames(params.Agents), ","))
	}
	if params.DaysOnly {
		q.Set("daysOnly", "1")
	}
	var out report.Report
	if err := s.Local.Get("/api/usage?"+q.Encode(), &out); err != nil {
		return report.Report{}, err
	}
	for i := range out.Models {
		for j, a := range out.Models[i].Agents {
			out.Models[i].Agents[j] = internalAgentName(a)
		}
	}
	return out, nil
}

func (s *SocketClient) UsageDay(date string, agents []string) (report.DayDetail, error) {
	q := url.Values{"date": {date}}
	if len(agents) > 0 {
		q.Set("agents", strings.Join(apiAgentNames(agents), ","))
	}
	var out report.DayDetail
	if err := s.Local.Get("/api/usage/day?"+q.Encode(), &out); err != nil {
		return report.DayDetail{}, err
	}
	for i := range out.Agents {
		out.Agents[i].Agent = internalAgentName(out.Agents[i].Agent)
	}
	return out, nil
}

func (s *SocketClient) UsageModel(model, from, to string, agents []string) (report.ModelDetail, error) {
	q := url.Values{"model": {model}, "from": {from}, "to": {to}}
	if len(agents) > 0 {
		q.Set("agents", strings.Join(apiAgentNames(agents), ","))
	}
	var out report.ModelDetail
	if err := s.Local.Get("/api/usage/model?"+q.Encode(), &out); err != nil {
		return report.ModelDetail{}, err
	}
	for i := range out.Agents {
		out.Agents[i].Agent = internalAgentName(out.Agents[i].Agent)
	}
	return out, nil
}

func (s *SocketClient) Quota() (rt.QuotaView, error) {
	var out rt.QuotaView
	if err := s.Local.Get("/api/quota", &out); err != nil {
		return rt.QuotaView{}, err
	}
	for i := range out.Providers {
		out.Providers[i].Provider = internalAgentName(out.Providers[i].Provider)
		if out.Providers[i].Snapshot != nil {
			out.Providers[i].Snapshot.Provider = internalAgentName(out.Providers[i].Snapshot.Provider)
		}
	}
	return out, nil
}

func (s *SocketClient) KeeperRecords(limit int) ([]keeper.ExecutionRecord, error) {
	var payload struct {
		Executions []struct {
			Time    time.Time `json:"time"`
			Agent   string    `json:"agent"`
			Trigger string    `json:"trigger"`
			Status  string    `json:"status"`
			Model   string    `json:"model"`
			Reply   string    `json:"reply"`
		} `json:"executions"`
	}
	if err := s.Local.Get("/api/keeper", &payload); err != nil {
		return nil, err
	}
	records := make([]keeper.ExecutionRecord, 0, len(payload.Executions))
	for _, ex := range payload.Executions {
		records = append(records, keeper.ExecutionRecord{
			Timestamp:     ex.Time,
			Agent:         internalAgentName(ex.Agent),
			Model:         ex.Model,
			Success:       ex.Status == "success",
			OutputSnippet: ex.Reply,
		})
	}
	if limit > 0 && len(records) > limit {
		records = records[:limit]
	}
	return records, nil
}

func (s *SocketClient) Refresh(usage, quota bool) (rt.RefreshResult, error) {
	var res rt.RefreshResult
	err := s.Local.Post("/api/refresh", map[string]bool{"usage": usage, "quota": quota}, &res)
	return res, err
}

func (s *SocketClient) PatchSettings(updates map[string]any) (config.Config, error) {
	if err := s.Local.Patch("/api/settings", updates, nil); err != nil {
		return config.Config{}, err
	}
	return s.Config()
}

func (s *SocketClient) KeeperPing(agent, group string) error {
	return s.Local.Post("/api/keeper/ping", map[string]any{"agent": apiAgentName(agent), "group": group}, nil)
}

func (s *SocketClient) Subscribe() (<-chan rt.Event, func(), error) {
	ch := make(chan rt.Event, 32)
	stop := make(chan struct{})
	var mu sync.Mutex
	var activeConn *websocket.Conn

	go func() {
		defer close(ch)
		for {
			select {
			case <-stop:
				return
			default:
			}
			conn, err := s.Local.WebSocket()
			if err != nil {
				select {
				case <-stop:
					return
				case <-time.After(2 * time.Second):
				}
				continue
			}
			mu.Lock()
			activeConn = conn
			mu.Unlock()
			for {
				_, data, err := conn.ReadMessage()
				if err != nil {
					break
				}
				var ev rt.Event
				if json.Unmarshal(data, &ev) == nil && ev.Type != "" {
					select {
					case ch <- ev:
					default:
					}
				}
			}
			mu.Lock()
			if activeConn == conn {
				activeConn = nil
			}
			mu.Unlock()
			_ = conn.Close()
		}
	}()

	var once sync.Once
	cancel := func() {
		once.Do(func() {
			close(stop)
			mu.Lock()
			if activeConn != nil {
				_ = activeConn.Close()
			}
			mu.Unlock()
		})
	}
	return ch, cancel, nil
}

func (s *SocketClient) IsRemote() bool { return true }

func (s *SocketClient) Close() error { return nil }
