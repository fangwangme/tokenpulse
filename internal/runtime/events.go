package runtime

import "sync"

// Event is a runtime state-change notification broadcast to local and remote
// frontends.
type Event struct {
	Type string `json:"type"`
	Data any    `json:"data"`
}

// Bus is a non-blocking fan-out event broadcaster. Slow subscribers drop
// events instead of stalling the runtime.
type Bus struct {
	mu   sync.RWMutex
	subs map[chan Event]struct{}
}

func NewBus() *Bus {
	return &Bus{subs: map[chan Event]struct{}{}}
}

func (b *Bus) Subscribe() (<-chan Event, func()) {
	ch := make(chan Event, 32)
	b.mu.Lock()
	b.subs[ch] = struct{}{}
	b.mu.Unlock()
	var once sync.Once
	cancel := func() {
		once.Do(func() {
			b.mu.Lock()
			delete(b.subs, ch)
			b.mu.Unlock()
		})
	}
	return ch, cancel
}

func (b *Bus) Publish(eventType string, data any) {
	event := Event{Type: eventType, Data: data}
	b.mu.RLock()
	for ch := range b.subs {
		select {
		case ch <- event:
		default:
		}
	}
	b.mu.RUnlock()
}
