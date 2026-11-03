package api

import (
	"fmt"
	"net/http"
	"net/netip"
	"strings"
)

// Proxies are the reverse proxies in front of the panel. Forwarding headers
// are only believed when the request comes from one of them.
type Proxies []netip.Prefix

// ParseProxies reads a comma-separated list of CIDRs or single addresses.
func ParseProxies(s string) (Proxies, error) {
	var out Proxies
	for _, field := range strings.Split(s, ",") {
		field = strings.TrimSpace(field)
		if field == "" {
			continue
		}
		prefix, err := netip.ParsePrefix(field)
		if err != nil {
			addr, addrErr := netip.ParseAddr(field)
			if addrErr != nil {
				return nil, fmt.Errorf("%q is not a CIDR or an IP address", field)
			}
			prefix = netip.PrefixFrom(addr, addr.BitLen())
		}
		out = append(out, prefix.Masked())
	}
	return out, nil
}

func (p Proxies) contains(ip netip.Addr) bool {
	for _, prefix := range p {
		if prefix.Contains(ip) {
			return true
		}
	}
	return false
}

// peer is the IP of the TCP peer.
func peer(r *http.Request) netip.Addr {
	addrPort, err := netip.ParseAddrPort(r.RemoteAddr)
	if err != nil {
		return netip.Addr{}
	}
	return addrPort.Addr().Unmap()
}

// clientIP is where the request came from: the peer, or behind trusted
// proxies the last X-Forwarded-For hop they didn't add themselves.
func (p Proxies) clientIP(r *http.Request) netip.Addr {
	ip := peer(r)
	if !p.contains(ip) {
		return ip
	}
	hops := strings.Split(strings.Join(r.Header.Values("X-Forwarded-For"), ","), ",")
	for i := len(hops) - 1; i >= 0; i-- {
		hop, err := netip.ParseAddr(strings.TrimSpace(hops[i]))
		if err != nil {
			break
		}
		ip = hop.Unmap()
		if !p.contains(ip) {
			break
		}
	}
	return ip
}

// forwarded returns the last value of a forwarding header set by a trusted
// proxy, or "" when there is none.
func (p Proxies) forwarded(r *http.Request, header string) string {
	if !p.contains(peer(r)) {
		return ""
	}
	values := strings.Split(strings.Join(r.Header.Values(header), ","), ",")
	return strings.TrimSpace(values[len(values)-1])
}

// host is the host (and port) the client asked for.
func (p Proxies) host(r *http.Request) string {
	if host := p.forwarded(r, "X-Forwarded-Host"); host != "" {
		return host
	}
	return r.Host
}

// https reports whether the client talks to the panel over HTTPS.
func (p Proxies) https(r *http.Request) bool {
	return r.TLS != nil || strings.EqualFold(p.forwarded(r, "X-Forwarded-Proto"), "https")
}
