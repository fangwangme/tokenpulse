package remote

import (
	"encoding/json"
	"fmt"
	"net"
	"net/url"
	"os/exec"
	"strconv"
	"strings"
)

type TailscaleInfo struct {
	DNSName string
	IPs     []string
}

func tailscaleStatus() (TailscaleInfo, bool) {
	out, err := exec.Command("tailscale", "status", "--json").Output()
	if err != nil {
		return TailscaleInfo{}, false
	}
	var parsed struct {
		Self struct {
			DNSName      string   `json:"DNSName"`
			TailscaleIPs []string `json:"TailscaleIPs"`
		} `json:"Self"`
	}
	if json.Unmarshal(out, &parsed) != nil || parsed.Self.DNSName == "" {
		return TailscaleInfo{}, false
	}
	return TailscaleInfo{
		DNSName: strings.TrimSuffix(parsed.Self.DNSName, "."),
		IPs:     parsed.Self.TailscaleIPs,
	}, true
}

// serveTarget reports what the HTTPS port proxies to. found=false means the
// port is unused.
func serveTarget(httpsPort int) (target string, found bool) {
	out, err := exec.Command("tailscale", "serve", "status", "--json").Output()
	if err != nil {
		return "", false
	}
	var parsed struct {
		Web map[string]struct {
			Handlers map[string]struct {
				Proxy string `json:"Proxy"`
			} `json:"Handlers"`
		} `json:"Web"`
	}
	if json.Unmarshal(out, &parsed) != nil {
		return "", false
	}
	suffix := ":" + strconv.Itoa(httpsPort)
	for key, entry := range parsed.Web {
		if !strings.HasSuffix(key, suffix) {
			continue
		}
		if handler, ok := entry.Handlers["/"]; ok {
			return handler.Proxy, true
		}
	}
	return "", false
}

func configureServe(httpsPort, localPort int) error {
	_, err := exec.Command("tailscale", "serve", "--bg", fmt.Sprintf("--https=%d", httpsPort), strconv.Itoa(localPort)).Output()
	return err
}

type Access struct {
	Mode     string
	URL      string
	HTTPS    bool
	BindHost string
	Hosts    map[string]bool
	Warning  string
}

func buildAccess(port, httpsPort int, noTailscale, lan bool) Access {
	hosts := map[string]bool{"localhost": true, "127.0.0.1": true, "::1": true}
	if lan {
		access := Access{Mode: "LAN", BindHost: "0.0.0.0", Hosts: hosts}
		for _, address := range interfaceAddresses() {
			access.Hosts[address] = true
		}
		if ip := firstNonLoopback(); ip != "" {
			access.URL = fmt.Sprintf("http://%s:%d/", ip, port)
		} else {
			access.URL = fmt.Sprintf("http://localhost:%d/", port)
		}
		access.Warning = "LAN traffic is unencrypted"
		return access
	}
	if noTailscale {
		return Access{Mode: "local", BindHost: "127.0.0.1", URL: fmt.Sprintf("http://localhost:%d/", port), Hosts: hosts}
	}
	info, ok := tailscaleStatus()
	if !ok {
		return Access{Mode: "local", BindHost: "127.0.0.1", URL: fmt.Sprintf("http://localhost:%d/", port), Hosts: hosts}
	}
	hosts[strings.ToLower(strings.TrimSuffix(info.DNSName, "."))] = true
	for _, ip := range info.IPs {
		hosts[ip] = true
	}
	if target, found := serveTarget(httpsPort); found {
		if isLocalServeTarget(target, port) {
			url := fmt.Sprintf("https://%s:%d/", info.DNSName, httpsPort)
			if httpsPort == 443 {
				url = fmt.Sprintf("https://%s/", info.DNSName)
			}
			return Access{Mode: "tailscale-serve", BindHost: "127.0.0.1", URL: url, HTTPS: true, Hosts: hosts}
		}
		return tailscaleIPAccess(info, port, hosts, fmt.Sprintf(
			"Tailscale Serve already maps HTTPS %d to %s; not overwriting it. "+
				"Run `tailscale serve --bg --https=%d %d` to use HTTPS.", httpsPort, target, httpsPort, port))
	}
	if err := configureServe(httpsPort, port); err != nil {
		return tailscaleIPAccess(info, port, hosts, fmt.Sprintf(
			"Tailscale Serve could not be configured: %v. Run `tailscale serve --bg --https=%d %d`.", err, httpsPort, port))
	}
	url := fmt.Sprintf("https://%s:%d/", info.DNSName, httpsPort)
	if httpsPort == 443 {
		url = fmt.Sprintf("https://%s/", info.DNSName)
	}
	return Access{Mode: "tailscale-serve", BindHost: "127.0.0.1", URL: url, HTTPS: true, Hosts: hosts}
}

// tailscaleIPAccess falls back to the Tailscale IP, but never to a wildcard
// bind: without an address the server stays on localhost.
func tailscaleIPAccess(info TailscaleInfo, port int, hosts map[string]bool, warning string) Access {
	ip := ""
	for _, candidate := range info.IPs {
		if strings.TrimSpace(candidate) != "" {
			ip = strings.TrimSpace(candidate)
			break
		}
	}
	if ip == "" {
		return Access{
			Mode: "local", BindHost: "127.0.0.1", URL: fmt.Sprintf("http://localhost:%d/", port), Hosts: hosts,
			Warning: warning + " No Tailscale IP is available; listening on localhost only.",
		}
	}
	return Access{
		Mode: "tailscale-ip", BindHost: ip, URL: fmt.Sprintf("http://%s:%d/", ip, port), Hosts: hosts,
		Warning: warning,
	}
}

func interfaceAddresses() []string {
	var out []string
	interfaces, err := net.Interfaces()
	if err != nil {
		return out
	}
	for _, iface := range interfaces {
		if iface.Flags&net.FlagUp == 0 || iface.Flags&net.FlagLoopback != 0 {
			continue
		}
		addresses, err := iface.Addrs()
		if err != nil {
			continue
		}
		for _, address := range addresses {
			ip, _, err := net.ParseCIDR(address.String())
			if err == nil && ip.To4() != nil {
				out = append(out, ip.String())
			}
		}
	}
	return out
}

func firstNonLoopback() string {
	addresses := interfaceAddresses()
	if len(addresses) == 0 {
		return ""
	}
	return addresses[0]
}

func isLocalServeTarget(target string, port int) bool {
	parsed, err := url.Parse(strings.TrimSpace(target))
	if err != nil || parsed.Host == "" {
		return false
	}
	host, targetPort, err := net.SplitHostPort(parsed.Host)
	if err != nil {
		return false
	}
	if targetPort != strconv.Itoa(port) {
		return false
	}
	host = strings.ToLower(strings.Trim(host, "[]"))
	return host == "127.0.0.1" || host == "localhost" || host == "::1"
}
