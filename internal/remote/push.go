package remote

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	webpush "github.com/SherClockHolmes/webpush-go"
)

var pushHTTPClient = &http.Client{Timeout: 15 * time.Second}

type pushStore struct {
	mu            sync.Mutex
	path          string
	vapidPath     string
	vapid         *vapidKeys
	subscriptions []json.RawMessage
}

type vapidKeys struct {
	PublicKey  string `json:"publicKey"`
	PrivateKey string `json:"privateKey"`
}

func newPushStore(dataDir string) *pushStore {
	return &pushStore{
		path:      filepath.Join(dataDir, "push-subscriptions.json"),
		vapidPath: filepath.Join(dataDir, "vapid.json"),
	}
}

func (p *pushStore) load() error {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.loadLocked()
}

func (p *pushStore) loadLocked() error {
	if p.vapid == nil {
		if content, err := os.ReadFile(p.vapidPath); err == nil {
			var keys vapidKeys
			if json.Unmarshal(content, &keys) == nil && keys.PrivateKey != "" {
				p.vapid = &keys
			}
		}
	}
	if p.vapid == nil {
		keys, err := generateVAPID()
		if err != nil {
			return err
		}
		p.vapid = keys
		data, _ := json.Marshal(keys)
		if err := os.WriteFile(p.vapidPath, data, 0o600); err != nil {
			return err
		}
	}
	if content, err := os.ReadFile(p.path); err == nil {
		var subscriptions []json.RawMessage
		if json.Unmarshal(content, &subscriptions) == nil {
			p.subscriptions = subscriptions
		}
	}
	return nil
}

func (p *pushStore) saveLocked() error {
	data, err := json.Marshal(p.subscriptions)
	if err != nil {
		return err
	}
	return os.WriteFile(p.path, data, 0o600)
}

func generateVAPID() (*vapidKeys, error) {
	private, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return nil, err
	}
	scalar := private.D.FillBytes(make([]byte, 32))
	public := elliptic.Marshal(private.Curve, private.PublicKey.X, private.PublicKey.Y)
	return &vapidKeys{
		PublicKey:  base64.RawURLEncoding.EncodeToString(public),
		PrivateKey: base64.RawURLEncoding.EncodeToString(scalar),
	}, nil
}

func (p *pushStore) PublicKey() (string, error) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if err := p.loadLocked(); err != nil {
		return "", err
	}
	return p.vapid.PublicKey, nil
}

func (p *pushStore) Add(subscription json.RawMessage) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if err := p.loadLocked(); err != nil {
		return err
	}
	var parsed struct {
		Endpoint string `json:"endpoint"`
	}
	if json.Unmarshal(subscription, &parsed) != nil || parsed.Endpoint == "" {
		return errors.New("invalid push subscription")
	}
	for index, existing := range p.subscriptions {
		var candidate struct {
			Endpoint string `json:"endpoint"`
		}
		if json.Unmarshal(existing, &candidate) == nil && candidate.Endpoint == parsed.Endpoint {
			// Browsers rotate the endpoint's auth/p256dh keys in place.
			p.subscriptions[index] = subscription
			return p.saveLocked()
		}
	}
	p.subscriptions = append(p.subscriptions, subscription)
	return p.saveLocked()
}

func (p *pushStore) Remove(endpoint string) error {
	p.mu.Lock()
	defer p.mu.Unlock()
	if err := p.loadLocked(); err != nil {
		return err
	}
	var kept []json.RawMessage
	for _, existing := range p.subscriptions {
		var candidate struct {
			Endpoint string `json:"endpoint"`
		}
		if json.Unmarshal(existing, &candidate) == nil && candidate.Endpoint == endpoint {
			continue
		}
		kept = append(kept, existing)
	}
	p.subscriptions = kept
	return p.saveLocked()
}

func pushPayload(title, body string) []byte {
	payload, _ := json.Marshal(map[string]string{"title": title, "body": body})
	return payload
}

// Send delivers one push; 404/410 drops the subscription. Callers run it in a
// goroutine so a slow push service cannot hold the refresh lock.
func (p *pushStore) Send(title, body string) {
	p.mu.Lock()
	if err := p.loadLocked(); err != nil {
		p.mu.Unlock()
		return
	}
	keys := *p.vapid
	subscriptions := append([]json.RawMessage(nil), p.subscriptions...)
	p.mu.Unlock()

	payload := pushPayload(title, body)
	for _, raw := range subscriptions {
		subscription := &webpush.Subscription{}
		if json.Unmarshal(raw, subscription) != nil {
			continue
		}
		resp, err := webpush.SendNotification(payload, subscription, &webpush.Options{
			HTTPClient:      pushHTTPClient,
			Subscriber:      "mailto:tokenpulse@localhost",
			VAPIDPublicKey:  keys.PublicKey,
			VAPIDPrivateKey: keys.PrivateKey,
			TTL:             60,
		})
		if err != nil {
			continue
		}
		if resp.StatusCode == 404 || resp.StatusCode == 410 {
			_ = p.Remove(subscription.Endpoint)
		}
		resp.Body.Close()
	}
}
