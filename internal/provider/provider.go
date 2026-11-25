// Package provider holds the message and quota shapes shared by every source.
package provider

import (
	"encoding/json"
	"time"
)

// TokenBreakdown is one message's token counts. Total is their sum, with
// reasoning counted only where the source reports it apart from output.
type TokenBreakdown struct {
	Input      int64 `json:"input"`
	Output     int64 `json:"output"`
	CacheRead  int64 `json:"cacheRead"`
	CacheWrite int64 `json:"cacheWrite"`
	Reasoning  int64 `json:"reasoning"`
}

func (t TokenBreakdown) Total() int64 {
	return t.Input + t.Output + t.CacheRead + t.CacheWrite + t.Reasoning
}

func (t TokenBreakdown) IsEmpty() bool { return t.Total() == 0 }

// UnifiedMessage is one ledger row before pricing.
type UnifiedMessage struct {
	Client        string
	ClientDetail  string
	ModelID       string
	ProviderID    string
	SessionID     string
	MessageKey    string
	Timestamp     int64 // ms since epoch
	Date          string
	Tokens        TokenBreakdown
	PricingDay    string
	ParserVersion string
	ServedModel   string
}

func NewMessage(client, modelID, providerID, sessionID, messageKey string, timestamp int64, tokens TokenBreakdown) UnifiedMessage {
	date := LocalDateString(timestamp)
	return UnifiedMessage{
		Client:        client,
		ModelID:       modelID,
		ProviderID:    providerID,
		SessionID:     sessionID,
		MessageKey:    messageKey,
		Timestamp:     timestamp,
		Date:          date,
		Tokens:        tokens,
		PricingDay:    date,
		ParserVersion: "v1",
	}
}

func (m UnifiedMessage) TotalTokens() int64 { return m.Tokens.Total() }

// LocalDateString formats a timestamp as the computer's local YYYY-MM-DD.
func LocalDateString(ms int64) string {
	return time.UnixMilli(ms).Local().Format("2006-01-02")
}

// QuotaSnapshot is one provider's quota reading.
type QuotaSnapshot struct {
	Provider              string                 `json:"provider"`
	Plan                  string                 `json:"plan,omitempty"`
	Account               string                 `json:"account,omitempty"`
	Windows               []RateWindow           `json:"windows"`
	Credits               *CreditInfo            `json:"credits,omitempty"`
	ExtraUsage            *CreditInfo            `json:"extraUsage,omitempty"`
	RateLimitResetCredits []RateLimitResetCredit `json:"rateLimitResetCredits,omitempty"`
	FetchedAt             time.Time              `json:"fetchedAt"`
}

// RateWindow is one quota window. ID exists for the API; the observation
// history keeps label and family.
type RateWindow struct {
	ID          string     `json:"id"`
	Label       string     `json:"label"`
	Family      string     `json:"family,omitempty"`
	UsedPercent float64    `json:"usedPercent"`
	PeriodMs    *int64     `json:"periodMs,omitempty"`
	ResetsAt    *time.Time `json:"resetsAt,omitempty"`
}

type CreditInfo struct {
	Used     float64  `json:"used"`
	Limit    *float64 `json:"limit,omitempty"`
	Currency string   `json:"currency"`
}

type RateLimitResetCredit struct {
	ID        string     `json:"id"`
	ResetType string     `json:"resetType,omitempty"`
	Status    string     `json:"status"`
	GrantedAt *time.Time `json:"grantedAt,omitempty"`
	ExpiresAt *time.Time `json:"expiresAt,omitempty"`
}

// cacheSnapshot mirrors the Rust serde field names stored in quota_cache.
type cacheSnapshot struct {
	Provider              string             `json:"provider"`
	Plan                  *string            `json:"plan"`
	Account               *string            `json:"account"`
	Windows               []cacheWindow      `json:"windows"`
	Credits               *cacheCredits      `json:"credits"`
	ExtraUsage            *cacheCredits      `json:"extra_usage"`
	RateLimitResetCredits []cacheResetCredit `json:"rate_limit_reset_credits"`
	FetchedAt             time.Time          `json:"fetched_at"`
}

type cacheWindow struct {
	Label            string     `json:"label"`
	ModelFamily      *string    `json:"model_family"`
	UsedPercent      float64    `json:"used_percent"`
	ResetsAt         *time.Time `json:"resets_at"`
	PeriodDurationMs *int64     `json:"period_duration_ms"`
}

type cacheCredits struct {
	Used     float64  `json:"used"`
	Limit    *float64 `json:"limit"`
	Currency string   `json:"currency"`
}

type cacheResetCredit struct {
	ID        string     `json:"id"`
	ResetType string     `json:"reset_type"`
	Status    string     `json:"status"`
	GrantedAt *time.Time `json:"granted_at"`
	ExpiresAt *time.Time `json:"expires_at"`
}

// MarshalCacheJSON writes the snapshot in the shape the Rust releases stored.
func (s QuotaSnapshot) MarshalCacheJSON() ([]byte, error) {
	out := cacheSnapshot{FetchedAt: s.FetchedAt}
	out.Provider = s.Provider
	if s.Plan != "" {
		p := s.Plan
		out.Plan = &p
	}
	if s.Account != "" {
		a := s.Account
		out.Account = &a
	}
	for _, w := range s.Windows {
		cw := cacheWindow{Label: w.Label, UsedPercent: w.UsedPercent, ResetsAt: w.ResetsAt, PeriodDurationMs: w.PeriodMs}
		if w.Family != "" {
			f := w.Family
			cw.ModelFamily = &f
		}
		out.Windows = append(out.Windows, cw)
	}
	if s.Credits != nil {
		out.Credits = &cacheCredits{Used: s.Credits.Used, Limit: s.Credits.Limit, Currency: s.Credits.Currency}
	}
	if s.ExtraUsage != nil {
		out.ExtraUsage = &cacheCredits{Used: s.ExtraUsage.Used, Limit: s.ExtraUsage.Limit, Currency: s.ExtraUsage.Currency}
	}
	for _, c := range s.RateLimitResetCredits {
		out.RateLimitResetCredits = append(out.RateLimitResetCredits, cacheResetCredit{
			ID: c.ID, ResetType: c.ResetType, Status: c.Status, GrantedAt: c.GrantedAt, ExpiresAt: c.ExpiresAt,
		})
	}
	return json.Marshal(out)
}

// UnmarshalCacheJSON reads either the Rust shape or the API shape.
func (s *QuotaSnapshot) UnmarshalCacheJSON(data []byte) error {
	var raw cacheSnapshot
	if err := json.Unmarshal(data, &raw); err == nil && raw.Provider != "" {
		*s = QuotaSnapshot{Provider: raw.Provider, Windows: []RateWindow{}, FetchedAt: raw.FetchedAt}
		if raw.Plan != nil {
			s.Plan = *raw.Plan
		}
		if raw.Account != nil {
			s.Account = *raw.Account
		}
		for _, w := range raw.Windows {
			rw := RateWindow{Label: w.Label, UsedPercent: w.UsedPercent, ResetsAt: w.ResetsAt, PeriodMs: w.PeriodDurationMs}
			if w.ModelFamily != nil {
				rw.Family = *w.ModelFamily
			}
			s.Windows = append(s.Windows, rw)
		}
		if raw.Credits != nil {
			s.Credits = &CreditInfo{Used: raw.Credits.Used, Limit: raw.Credits.Limit, Currency: raw.Credits.Currency}
		}
		if raw.ExtraUsage != nil {
			s.ExtraUsage = &CreditInfo{Used: raw.ExtraUsage.Used, Limit: raw.ExtraUsage.Limit, Currency: raw.ExtraUsage.Currency}
		}
		for _, c := range raw.RateLimitResetCredits {
			s.RateLimitResetCredits = append(s.RateLimitResetCredits, RateLimitResetCredit{
				ID: c.ID, ResetType: c.ResetType, Status: c.Status, GrantedAt: c.GrantedAt, ExpiresAt: c.ExpiresAt,
			})
		}
		return nil
	}
	return json.Unmarshal(data, s)
}
