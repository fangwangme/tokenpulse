package usage

import (
	"time"
)

// AntigravityConnectionInfo is a detected Antigravity language server, exposed
// for the quota fetcher.
type AntigravityConnectionInfo struct {
	PID     int
	Port    int
	CSRF    string
	Scheme  string
	Runtime string
}

// DetectAntigravityConnectionsForQuota finds running language servers.
func DetectAntigravityConnectionsForQuota() ([]AntigravityConnectionInfo, error) {
	connections, err := detectAntigravityConnections()
	if err != nil {
		return nil, err
	}
	out := make([]AntigravityConnectionInfo, 0, len(connections))
	for _, connection := range connections {
		out = append(out, AntigravityConnectionInfo{
			PID: connection.PID, Port: connection.Port, CSRF: connection.CSRF,
			Scheme: connection.Scheme, Runtime: connection.Runtime.asString(),
		})
	}
	return out, nil
}

// AntigravityRPC posts one JSON request to a language server.
func AntigravityRPC(connection AntigravityConnectionInfo, method string, body any, timeout time.Duration) (map[string]any, error) {
	return rpcRequest(antigravityConnection{
		PID: connection.PID, Port: connection.Port, CSRF: connection.CSRF,
		Scheme: connection.Scheme, Runtime: antigravityRuntimeFromString(connection.Runtime),
	}, method, body, timeout)
}

func antigravityRuntimeFromString(value string) antigravityRuntime {
	switch value {
	case "desktop":
		return agyDesktop
	case "ide":
		return agyIDE
	case "cli":
		return agyCLI
	default:
		return agyUnknown
	}
}
