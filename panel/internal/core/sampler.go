package core

import (
	"context"
	"sync"
	"time"
)

const (
	SampleInterval = 5 * time.Second
	// An hour of samples.
	maxSamples = 720
)

// Sample is the core's traffic over one sampling interval.
type Sample struct {
	T           int64   `json:"t"`
	UpBps       float64 `json:"up_bps"`
	DownBps     float64 `json:"down_bps"`
	Connections uint64  `json:"connections"`
}

// Sampler polls the core's status and turns its byte counters into rates.
type Sampler struct {
	status   func(context.Context) (*Status, error)
	interval time.Duration
	// Called with every status the core returns.
	onStatus func(context.Context, *Status)

	mu      sync.Mutex
	samples []Sample
	prev    *Status
	prevAt  time.Time
}

// NewSampler polls status every interval. onStatus may be nil.
func NewSampler(status func(context.Context) (*Status, error), interval time.Duration,
	onStatus func(context.Context, *Status)) *Sampler {
	return &Sampler{status: status, interval: interval, onStatus: onStatus}
}

// Run polls until ctx ends.
func (s *Sampler) Run(ctx context.Context) {
	ticker := time.NewTicker(s.interval)
	defer ticker.Stop()
	for {
		s.poll(ctx)
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
	}
}

func (s *Sampler) poll(ctx context.Context) {
	ctx, cancel := context.WithTimeout(ctx, s.interval)
	defer cancel()
	st, err := s.status(ctx)
	if err != nil {
		// A gap in the history; the next rate starts from fresh counters.
		s.mu.Lock()
		s.prev = nil
		s.mu.Unlock()
		return
	}
	s.add(st, time.Now())
	if s.onStatus != nil {
		s.onStatus(ctx, st)
	}
}

func (s *Sampler) add(st *Status, now time.Time) {
	s.mu.Lock()
	defer s.mu.Unlock()
	prev, prevAt := s.prev, s.prevAt
	s.prev, s.prevAt = st, now
	if prev == nil {
		return
	}
	elapsed := now.Sub(prevAt).Seconds()
	if elapsed <= 0 {
		return
	}
	// A restarted core counts from zero again.
	restarted := st.UptimeSecs < prev.UptimeSecs
	s.samples = append(s.samples, Sample{
		T:           now.Unix(),
		UpBps:       float64(counterDelta(prev.BytesUp, st.BytesUp, restarted)) / elapsed,
		DownBps:     float64(counterDelta(prev.BytesDown, st.BytesDown, restarted)) / elapsed,
		Connections: st.ActiveConnections,
	})
	if len(s.samples) > maxSamples {
		s.samples = append(s.samples[:0], s.samples[len(s.samples)-maxSamples:]...)
	}
}

// counterDelta is how much a counter grew. Counters also go down when traffic
// is reset or a node is deleted; that interval then counts as idle.
func counterDelta(prev, cur uint64, restarted bool) uint64 {
	switch {
	case restarted:
		return cur
	case cur < prev:
		return 0
	default:
		return cur - prev
	}
}

// History returns the samples of the last hour, oldest first.
func (s *Sampler) History() []Sample {
	s.mu.Lock()
	defer s.mu.Unlock()
	return append([]Sample{}, s.samples...)
}
