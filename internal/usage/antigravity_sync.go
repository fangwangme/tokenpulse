package usage

import (
	"bytes"
	"crypto/tls"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

const antigravityLSService = "exa.language_server_pb.LanguageServerService"

type antigravityRuntime string

const (
	agyDesktop antigravityRuntime = "Desktop"
	agyIDE     antigravityRuntime = "IDE"
	agyCLI     antigravityRuntime = "CLI"
	agyUnknown antigravityRuntime = "Unknown"
)

func (r antigravityRuntime) asString() string {
	switch r {
	case agyDesktop:
		return "desktop"
	case agyIDE:
		return "ide"
	case agyCLI:
		return "cli"
	default:
		return "unknown"
	}
}

func runtimeClient(r antigravityRuntime) string {
	switch r {
	case agyDesktop:
		return "antigravity-desktop"
	case agyIDE:
		return "antigravity-ide"
	case agyCLI:
		return "antigravity-cli"
	default:
		return "antigravity"
	}
}

type antigravityConnection struct {
	PID     int
	Port    int
	CSRF    string
	Scheme  string
	Runtime antigravityRuntime
}

type processCandidate struct {
	pid          int
	ppid         int
	declaredPort int
	csrf         string
	runtime      antigravityRuntime
}

func runCommand(name string, args ...string) (string, error) {
	out, err := exec.Command(name, args...).Output()
	return string(out), err
}

func detectAntigravityConnections() ([]antigravityConnection, error) {
	candidates, err := detectProcessCandidates()
	if err != nil {
		return nil, err
	}
	var connections []antigravityConnection
	for _, candidate := range candidates {
		ports := candidateProbePorts(candidate, findListeningPorts(candidate.pid))
		preferred := "https"
		if candidate.runtime == agyCLI {
			preferred = "http"
		}
		for _, port := range ports {
			if scheme := probeHeartbeat(port, candidate.csrf, preferred); scheme != "" {
				connections = append(connections, antigravityConnection{
					PID: candidate.pid, Port: port, CSRF: candidate.csrf,
					Scheme: scheme, Runtime: candidate.runtime,
				})
				break
			}
		}
	}
	if len(connections) == 0 {
		if address := os.Getenv("ANTIGRAVITY_LS_ADDRESS"); address != "" {
			port, scheme := parseLSAddress(address)
			if port > 0 {
				if preferred := scheme; preferred != "" {
					if found := probeHeartbeat(port, os.Getenv("ANTIGRAVITY_CSRF_TOKEN"), preferred); found != "" {
						connections = append(connections, antigravityConnection{
							PID: os.Getpid(), Port: port, CSRF: os.Getenv("ANTIGRAVITY_CSRF_TOKEN"),
							Scheme: found, Runtime: agyCLI,
						})
					}
				}
			}
		}
	}
	sort.Slice(connections, func(i, j int) bool {
		if connections[i].PID != connections[j].PID {
			return connections[i].PID > connections[j].PID
		}
		return connections[i].Port < connections[j].Port
	})
	deduped := connections[:0]
	for _, connection := range connections {
		duplicate := false
		for _, existing := range deduped {
			if existing.PID == connection.PID && existing.Port == connection.Port {
				duplicate = true
				break
			}
		}
		if !duplicate {
			deduped = append(deduped, connection)
		}
	}
	return deduped, nil
}

func detectProcessCandidates() ([]processCandidate, error) {
	output, err := runCommand("ps", "-ww", "-eo", "pid,ppid,args")
	if err != nil {
		return nil, err
	}
	var candidates []processCandidate
	children := map[int][]int{}
	for _, line := range strings.Split(output, "\n") {
		fields := strings.Fields(strings.TrimSpace(line))
		if len(fields) < 3 {
			continue
		}
		pid, err1 := strconv.Atoi(fields[0])
		ppid, err2 := strconv.Atoi(fields[1])
		if err1 != nil || err2 != nil {
			continue
		}
		command := strings.Join(fields[2:], " ")
		children[ppid] = append(children[ppid], pid)
		if !isAntigravityProcess(command) || isElectronHelper(command) {
			continue
		}
		exePath := processExecutablePath(pid)
		if exePath != "" {
			lower := strings.ToLower(exePath)
			if !strings.Contains(lower, "antigravity") && !strings.Contains(lower, "language_server") &&
				!strings.EqualFold(filepath.Base(exePath), "agy") {
				continue
			}
		}
		candidates = append(candidates, processCandidate{
			pid: pid, ppid: ppid,
			declaredPort: extractDeclaredPort(command),
			csrf:         extractCSRFToken(command),
			runtime:      inferRuntimeKind(command, exePath),
		})
	}
	for i := range candidates {
		c := &candidates[i]
		if c.runtime != agyCLI {
			continue
		}
		if c.csrf == "" {
			c.csrf = strings.TrimSpace(os.Getenv("ANTIGRAVITY_CSRF_TOKEN"))
		}
		if c.declaredPort == 0 {
			if port, _ := parseLSAddress(os.Getenv("ANTIGRAVITY_LS_ADDRESS")); port > 0 {
				c.declaredPort = port
			}
		}
		if c.csrf == "" || c.declaredPort == 0 {
			targets := append([]int{c.pid}, children[c.pid]...)
			for _, target := range targets {
				if token, port := inspectPIDEnv(target); token != "" || port > 0 {
					if c.csrf == "" {
						c.csrf = token
					}
					if c.declaredPort == 0 {
						c.declaredPort = port
					}
					break
				}
			}
		}
	}
	sort.Slice(candidates, func(i, j int) bool { return candidates[i].pid > candidates[j].pid })
	deduped := candidates[:0]
	seen := map[int]bool{}
	for _, candidate := range candidates {
		if seen[candidate.pid] {
			continue
		}
		seen[candidate.pid] = true
		deduped = append(deduped, candidate)
	}
	return deduped, nil
}

func candidateProbePorts(candidate processCandidate, ports []int) []int {
	if candidate.declaredPort > 0 {
		out := []int{candidate.declaredPort}
		for _, port := range ports {
			if port != candidate.declaredPort {
				out = append(out, port)
			}
		}
		return out
	}
	sort.Ints(ports)
	deduped := ports[:0]
	for i, port := range ports {
		if i == 0 || port != ports[i-1] {
			deduped = append(deduped, port)
		}
	}
	return deduped
}

func inspectPIDEnv(pid int) (string, int) {
	data, err := os.ReadFile(fmt.Sprintf("/proc/%d/environ", pid))
	if err != nil {
		return "", 0
	}
	var token, address string
	for _, chunk := range bytes.Split(data, []byte{0}) {
		s := string(chunk)
		if strings.HasPrefix(s, "ANTIGRAVITY_CSRF_TOKEN=") {
			token = strings.TrimSpace(strings.TrimPrefix(s, "ANTIGRAVITY_CSRF_TOKEN="))
		} else if strings.HasPrefix(s, "ANTIGRAVITY_LS_ADDRESS=") {
			address = strings.TrimSpace(strings.TrimPrefix(s, "ANTIGRAVITY_LS_ADDRESS="))
		}
	}
	port, _ := parseLSAddress(address)
	return token, port
}

func parseLSAddress(address string) (int, string) {
	trimmed := strings.TrimSpace(address)
	scheme := ""
	rest := trimmed
	if strings.HasPrefix(trimmed, "https://") {
		scheme, rest = "https", strings.TrimPrefix(trimmed, "https://")
	} else if strings.HasPrefix(trimmed, "http://") {
		scheme, rest = "http", strings.TrimPrefix(trimmed, "http://")
	}
	portText := rest
	if idx := strings.LastIndex(rest, ":"); idx >= 0 {
		portText = rest[idx+1:]
	}
	port, err := strconv.Atoi(portText)
	if err != nil {
		return 0, scheme
	}
	return port, scheme
}

func isElectronHelper(command string) bool {
	for _, token := range strings.Fields(command) {
		switch token {
		case "--type=renderer", "--type=gpu-process", "--type=utility", "--type=zygote", "--type=broker":
			return true
		}
	}
	return false
}

func isAntigravityProcess(command string) bool {
	lower := strings.ToLower(command)
	first := command
	if fields := strings.Fields(command); len(fields) > 0 {
		first = fields[0]
	}
	return (strings.Contains(lower, "language_server") &&
		(strings.Contains(lower, "antigravity") ||
			(strings.Contains(lower, "--app_data_dir") && strings.Contains(lower, "antigravity")))) ||
		strings.Contains(lower, "/antigravity/") || strings.Contains(lower, "\\antigravity\\") ||
		strings.Contains(lower, "/antigravity-cli/") || strings.Contains(lower, "\\antigravity-cli\\") ||
		strings.Contains(lower, "/antigravity-ide/") || strings.Contains(lower, "\\antigravity-ide\\") ||
		strings.Contains(lower, "antigravity.app") || strings.Contains(lower, "antigravity ide.app") ||
		strings.EqualFold(filepath.Base(first), "agy")
}

func inferRuntimeKind(command, exePath string) antigravityRuntime {
	if appData := extractFlagValue(command, "--app_data_dir"); appData != "" {
		switch strings.ToLower(strings.TrimSpace(appData)) {
		case "antigravity-cli":
			return agyCLI
		case "antigravity-ide":
			return agyIDE
		case "antigravity":
			return agyDesktop
		}
	}
	lower := strings.ToLower(command)
	exeLower := strings.ToLower(exePath)
	first := command
	if fields := strings.Fields(command); len(fields) > 0 {
		first = fields[0]
	}
	switch {
	case strings.Contains(lower, "antigravity-cli"), strings.Contains(exeLower, "antigravity-cli"),
		strings.EqualFold(filepath.Base(first), "agy"), strings.EqualFold(filepath.Base(exePath), "agy"):
		return agyCLI
	case strings.Contains(lower, "antigravity-ide"), strings.Contains(exeLower, "antigravity-ide"),
		strings.Contains(lower, "antigravity ide.app"), strings.Contains(exeLower, "antigravity ide.app"):
		return agyIDE
	case strings.Contains(lower, "/antigravity/"), strings.Contains(lower, "\\antigravity\\"),
		strings.Contains(lower, "antigravity.app"), strings.Contains(exeLower, "antigravity.app"):
		return agyDesktop
	default:
		return agyUnknown
	}
}

func processExecutablePath(pid int) string {
	out, err := runCommand("lsof", "-p", strconv.Itoa(pid), "-Fn")
	if err != nil {
		return ""
	}
	for _, line := range strings.Split(out, "\n") {
		if strings.HasPrefix(line, "n") && strings.Contains(line, ".app/Contents/MacOS/") {
			return strings.TrimPrefix(line, "n")
		}
	}
	return ""
}

func extractCSRFToken(command string) string {
	token := extractFlagValue(command, "--csrf_token")
	if len(token) < 32 {
		return ""
	}
	for _, r := range token {
		if !(r >= '0' && r <= '9' || r >= 'a' && r <= 'f' || r >= 'A' && r <= 'F' || r == '-') {
			return ""
		}
	}
	return token
}

func extractDeclaredPort(command string) int {
	value := extractFlagValue(command, "--extension_server_port")
	port, err := strconv.Atoi(value)
	if err != nil {
		return 0
	}
	return port
}

func extractFlagValue(command, flag string) string {
	compact := flag + "="
	tokens := strings.Fields(command)
	for _, token := range tokens {
		if strings.HasPrefix(token, compact) {
			return unquoteFlag(strings.TrimPrefix(token, compact))
		}
	}
	for i, token := range tokens {
		if token == flag && i+1 < len(tokens) {
			return unquoteFlag(tokens[i+1])
		}
	}
	return ""
}

func unquoteFlag(value string) string {
	return strings.Trim(strings.TrimSpace(value), "'\"")
}

func findListeningPorts(pid int) []int {
	ports := runPortQuery("lsof", "-Pan", "-p", strconv.Itoa(pid), "-iTCP", "-sTCP:LISTEN")
	if len(ports) == 0 {
		ports = runPortQuery("lsof", "-Pan", "-p", strconv.Itoa(pid), "-i")
	}
	sort.Ints(ports)
	return ports
}

func runPortQuery(program string, args ...string) []int {
	out, err := runCommand(program, args...)
	if err != nil {
		return nil
	}
	var ports []int
	for _, line := range strings.Split(out, "\n") {
		if port := parsePortFromLine(line); port > 0 {
			ports = append(ports, port)
		}
	}
	return ports
}

func parsePortFromLine(line string) int {
	for _, token := range strings.Fields(line) {
		for _, prefix := range []string{"127.0.0.1:", "localhost:", "*:", "::1:"} {
			if strings.HasPrefix(token, prefix) {
				text := strings.TrimSuffix(strings.TrimSuffix(strings.TrimPrefix(token, prefix), "(LISTEN)"), ",")
				if port, err := strconv.Atoi(text); err == nil {
					return port
				}
			}
		}
	}
	if idx := strings.LastIndex(line, ":"); idx >= 0 {
		rest := strings.TrimSpace(line[idx+1:])
		digits := ""
		for _, r := range rest {
			if r < '0' || r > '9' {
				break
			}
			digits += string(r)
		}
		if port, err := strconv.Atoi(digits); err == nil {
			return port
		}
	}
	return 0
}

var insecureClient = &http.Client{
	Timeout: 25 * time.Second,
	Transport: &http.Transport{
		TLSClientConfig: &tls.Config{InsecureSkipVerify: true},
		Proxy:           http.ProxyFromEnvironment,
	},
}

func probeHeartbeat(port int, csrf, preferred string) string {
	schemes := []string{"https", "http"}
	if preferred == "http" {
		schemes = []string{"http", "https"}
	}
	for _, scheme := range schemes {
		status, text, err := languageServerRequest(scheme, port, csrf, "Heartbeat",
			map[string]any{"uuid": "00000000-0000-0000-0000-000000000000"}, time.Second)
		if err != nil {
			continue
		}
		if status == 401 && (strings.Contains(text, "CSRF token") || strings.Contains(text, "unauthenticated")) {
			continue
		}
		if status == 200 && heartbeatWellFormed(text) && probeEndpointIdentity(scheme, port, csrf) {
			return scheme
		}
	}
	return ""
}

func heartbeatWellFormed(body string) bool {
	trimmed := strings.TrimLeft(body, " \t\r\n")
	idx := strings.IndexAny(trimmed, "{[")
	if idx < 0 {
		return false
	}
	var value any
	return json.Unmarshal([]byte(trimmed[idx:]), &value) == nil
}

func probeEndpointIdentity(scheme string, port int, csrf string) bool {
	for _, method := range []string{"GetCascadeTrajectoryGeneratorMetadata", "GetAllCascadeTrajectories"} {
		status, text, err := languageServerRequest(scheme, port, csrf, method, map[string]any{}, time.Second)
		if err != nil {
			continue
		}
		if status != 200 && !(status == 500 && strings.Contains(text, "trajectory not found")) {
			continue
		}
		if responseContainsAntigravityMarker(method, text) {
			return true
		}
	}
	return false
}

func responseContainsAntigravityMarker(method, body string) bool {
	trimmed := strings.TrimLeft(body, " \t\r\n")
	idx := strings.IndexAny(trimmed, "{[")
	if idx < 0 {
		return false
	}
	trimmed = trimmed[idx:]
	var value any
	if err := json.Unmarshal([]byte(trimmed), &value); err != nil {
		return prefixContainsMarker(trimmed)
	}
	if method == "GetAllCascadeTrajectories" {
		switch v := value.(type) {
		case []any:
			if len(v) == 0 {
				return true
			}
		case map[string]any:
			if len(v) == 0 {
				return true
			}
		}
	}
	return containsMarker(value)
}

func prefixContainsMarker(body string) bool {
	if !strings.HasPrefix(body, "{") && !strings.HasPrefix(body, "[") {
		return false
	}
	for _, marker := range []string{"\"cascadeId\"", "\"cascadeTrajectories\"", "\"trajectorySummaries\"",
		"\"generatorMetadata\"", "\"serverInfo\"", "\"serverCapabilities\""} {
		for _, suffix := range strings.Split(body, marker)[1:] {
			if strings.HasPrefix(strings.TrimLeft(suffix, " \t\r\n"), ":") {
				return true
			}
		}
	}
	return false
}

func containsMarker(value any) bool {
	markers := []string{"cascadeId", "cascadeTrajectories", "trajectorySummaries", "generatorMetadata",
		"serverInfo", "serverCapabilities", "trajectoryId", "trajectoryMetadata", "trajectoryType"}
	switch v := value.(type) {
	case map[string]any:
		for key, child := range v {
			for _, marker := range markers {
				if strings.EqualFold(key, marker) {
					return true
				}
			}
			if containsMarker(child) {
				return true
			}
		}
	case []any:
		for _, child := range v {
			if containsMarker(child) {
				return true
			}
		}
	case string:
		lower := strings.ToLower(v)
		return strings.Contains(lower, "trajectory") || strings.Contains(lower, "generatormetadata")
	}
	return false
}

func languageServerRequest(scheme string, port int, csrf, method string, body any, timeout time.Duration) (int, string, error) {
	data, err := json.Marshal(body)
	if err != nil {
		return 0, "", err
	}
	url := fmt.Sprintf("%s://127.0.0.1:%d/%s/%s", scheme, port, antigravityLSService, method)
	req, err := http.NewRequest(http.MethodPost, url, bytes.NewReader(data))
	if err != nil {
		return 0, "", err
	}
	req.Header.Set("Content-Type", "application/json")
	req.Header.Set("Connect-Protocol-Version", "1")
	if csrf != "" {
		req.Header.Set("X-Codeium-Csrf-Token", csrf)
	}
	client := *insecureClient
	client.Timeout = timeout
	resp, err := client.Do(req)
	if err != nil {
		return 0, "", err
	}
	defer resp.Body.Close()
	text, _ := io.ReadAll(io.LimitReader(resp.Body, 4*1024*1024))
	return resp.StatusCode, string(text), nil
}

func rpcRequest(connection antigravityConnection, method string, body any, timeout time.Duration) (map[string]any, error) {
	status, text, err := languageServerRequest(connection.Scheme, connection.Port, connection.CSRF, method, body, timeout)
	if err != nil {
		return nil, err
	}
	if status != 200 {
		return nil, fmt.Errorf("HTTP %d", status)
	}
	var value map[string]any
	if err := json.Unmarshal([]byte(text), &value); err != nil {
		return nil, err
	}
	return value, nil
}

func fetchDynamicModelAliases(connections []antigravityConnection) map[string]ModelAlias {
	aliases := map[string]ModelAlias{}
	body := map[string]any{"metadata": map[string]any{
		"ideName": "antigravity", "extensionName": "antigravity", "ideVersion": "unknown", "locale": "en",
	}}
	for _, connection := range connections {
		response, err := rpcRequest(connection, "GetUserStatus", body, 10*time.Second)
		if err != nil {
			continue
		}
		collectModelAliases(response, aliases)
	}
	return aliases
}

func collectModelAliases(value map[string]any, aliases map[string]ModelAlias) {
	if configs, ok := value["clientModelConfigs"].([]any); ok {
		collectModelAliasConfigs(configs, aliases)
	}
	if data, ok := value["cascadeModelConfigData"].(map[string]any); ok {
		if configs, ok := data["clientModelConfigs"].([]any); ok {
			collectModelAliasConfigs(configs, aliases)
		}
	}
	if status, ok := value["userStatus"].(map[string]any); ok {
		if data, ok := status["cascadeModelConfigData"].(map[string]any); ok {
			if configs, ok := data["clientModelConfigs"].([]any); ok {
				collectModelAliasConfigs(configs, aliases)
			}
		}
	}
}

func collectModelAliasConfigs(configs []any, aliases map[string]ModelAlias) {
	for _, raw := range configs {
		config, _ := raw.(map[string]any)
		if config == nil {
			continue
		}
		modelOrAlias, _ := config["modelOrAlias"].(map[string]any)
		rawModel, _ := modelOrAlias["model"].(string)
		if rawModel == "" || !strings.HasPrefix(strings.ToLower(strings.TrimSpace(rawModel)), "model") {
			continue
		}
		label, _ := config["label"].(string)
		if strings.TrimSpace(label) == "" {
			continue
		}
		modelID, ok := labelToModelID(label)
		if !ok {
			continue
		}
		aliases[normalizeAliasKey(rawModel)] = ModelAlias{
			RawModelID: rawModel, ModelID: modelID, Label: label, Source: "antigravity-get-user-status",
		}
	}
}

type localConversationID struct {
	sessionID  string
	modifiedMs *int64
	runtime    antigravityRuntime
}

type antigravityWorkItem struct {
	sessionID  string
	canonical  antigravityRuntime
	candidates []antigravityRuntime
	summary    antigravitySyncSummary
}

func discoverLocalConversationIDs(home string) []localConversationID {
	dirs := []struct {
		path    string
		runtime antigravityRuntime
	}{
		{filepath.Join(home, ".gemini", "antigravity", "conversations"), agyDesktop},
		{filepath.Join(home, ".gemini", "antigravity-ide", "conversations"), agyIDE},
		{filepath.Join(home, ".gemini", "antigravity-cli", "conversations"), agyCLI},
	}
	var ids []localConversationID
	for _, dir := range dirs {
		entries, err := os.ReadDir(dir.path)
		if err != nil {
			continue
		}
		for _, entry := range entries {
			if entry.IsDir() {
				continue
			}
			ext := strings.TrimPrefix(filepath.Ext(entry.Name()), ".")
			if ext != "pb" && ext != "db" {
				continue
			}
			stem := strings.TrimSuffix(entry.Name(), "."+ext)
			if len(stem) < 20 {
				continue
			}
			if ms, ok := localConversationModifiedMs(filepath.Join(dir.path, entry.Name())); ok {
				ids = append(ids, localConversationID{sessionID: stem, modifiedMs: &ms, runtime: dir.runtime})
			} else {
				ids = append(ids, localConversationID{sessionID: stem, runtime: dir.runtime})
			}
		}
	}
	sort.Slice(ids, func(i, j int) bool {
		if ids[i].sessionID != ids[j].sessionID {
			return ids[i].sessionID < ids[j].sessionID
		}
		return ids[i].runtime.asString() < ids[j].runtime.asString()
	})
	return ids
}

func syncLanguageServers(db *sql.DB, home string, connections []antigravityConnection,
	cached map[[2]string][2]*int64, rebuild bool, aliases map[string]ModelAlias, releases ReleaseMap) error {
	locals := discoverLocalConversationIDs(home)
	work := map[[2]string]*antigravityWorkItem{}
	for _, local := range locals {
		key := [2]string{local.sessionID, string(local.runtime)}
		item, ok := work[key]
		if !ok {
			item = &antigravityWorkItem{sessionID: local.sessionID, canonical: local.runtime, candidates: []antigravityRuntime{local.runtime}}
			work[key] = item
		}
		item.summary.LastModifiedMs = newerInt64(item.summary.LastModifiedMs, local.modifiedMs)
	}
	hasLocal := map[antigravityRuntime]bool{}
	for _, local := range locals {
		hasLocal[local.runtime] = true
	}
	for _, connection := range connections {
		if !rebuild && hasLocal[connection.Runtime] {
			continue
		}
		response, err := rpcRequest(connection, "GetAllCascadeTrajectories", map[string]any{}, 10*time.Second)
		if err != nil {
			continue
		}
		for _, entry := range extractTrajectoryEntries(response) {
			sessionID := entry.key
			if sessionID == "" {
				sessionID = firstString(entry.item, "cascadeId", "trajectoryId", "id", "sessionId")
			}
			if sessionID == "" {
				continue
			}
			summary := summaryFromTrajectory(entry.item)
			workKey := [2]string{sessionID, string(connection.Runtime)}
			if existing, ok := work[workKey]; ok {
				enrichSummary(&existing.summary, summary)
			} else {
				work[workKey] = &antigravityWorkItem{
					sessionID: sessionID, canonical: connection.Runtime,
					candidates: []antigravityRuntime{connection.Runtime}, summary: summary,
				}
			}
		}
	}
	collapseDesktopIDEOverlap(work)

	now := time.Now().UnixMilli()
	for _, item := range work {
		candidates := resolveCandidateConnections(item.candidates, connections)
		if len(candidates) == 0 {
			continue
		}
		client := runtimeClient(item.canonical)
		if !rebuild {
			if cachedEntry, ok := cached[[2]string{client, item.sessionID}]; ok {
				unchanged := false
				if cachedEntry[0] != nil && item.summary.LastModifiedMs != nil {
					unchanged = *cachedEntry[0] >= *item.summary.LastModifiedMs
				} else if cachedEntry[1] != nil && item.summary.StepCount != nil {
					unchanged = *cachedEntry[1] >= *item.summary.StepCount
				}
				if unchanged {
					continue
				}
			}
		}
		var metadata []any
		var obtained bool
		for _, connection := range candidates {
			response, err := rpcRequest(connection, "GetCascadeTrajectoryGeneratorMetadata",
				map[string]any{"cascadeId": item.sessionID}, 10*time.Second)
			if err != nil {
				continue
			}
			if raw, ok := response["generatorMetadata"].([]any); ok {
				metadata = raw
				obtained = true
				break
			}
		}
		if !obtained {
			if _, ok := cached[[2]string{client, item.sessionID}]; !ok {
				_ = upsertAntigravitySession(db, item.sessionID, client, item.summary, "unknown", now)
			}
			continue
		}
		primary := "unknown"
		for _, raw := range metadata {
			meta, _ := raw.(map[string]any)
			chatModel := meta
			if inner, ok := meta["chatModel"].(map[string]any); ok {
				chatModel = inner
			}
			resolved := resolveRPCChatModel(chatModel, aliases, releases, providerDateNow())
			if resolved != "unknown" {
				primary = resolved
				break
			}
		}
		if err := upsertAntigravitySession(db, item.sessionID, client, item.summary, primary, now); err != nil {
			continue
		}
		if err := writeRPCSessionUsage(db, home, item.sessionID, client, item.candidates, metadata, aliases, releases); err != nil {
			return err
		}
	}
	return nil
}

func resolveRPCChatModel(chatModel map[string]any, aliases map[string]ModelAlias, releases ReleaseMap, fallbackDate string) string {
	enum, _ := chatModel["model"].(string)
	served, _ := chatModel["responseModel"].(string)
	modelID := ResolveAntigravityModel(enum, served, aliases, releases, fallbackDate)
	if modelid.IsPseudo(modelID) {
		if label, ok := chatModel["modelDisplayName"].(string); ok {
			if id, ok := labelToModelID(label); ok {
				modelID = id
			}
		}
	}
	return modelID
}

func writeRPCSessionUsage(db *sql.DB, home, sessionID, client string, candidates []antigravityRuntime,
	metadata []any, aliases map[string]ModelAlias, releases ReleaseMap) error {
	for _, runtime := range candidates {
		if _, err := db.Exec("DELETE FROM session_usage WHERE client = ? AND session_id = ?", runtimeClient(runtime), sessionID); err != nil {
			return err
		}
	}
	var localTimestamps map[string]int64
	for stepIndex, raw := range metadata {
		meta, _ := raw.(map[string]any)
		if meta == nil {
			continue
		}
		chatModel := meta
		if inner, ok := meta["chatModel"].(map[string]any); ok {
			chatModel = inner
		}
		var createdAt *int64
		if startMeta, ok := chatModel["chatStartMetadata"].(map[string]any); ok {
			createdAt = parseTimestampValue(startMeta["createdAt"])
		}
		retryInfos, ok := chatModel["retryInfos"].([]any)
		if !ok {
			continue
		}
		for _, retryRaw := range retryInfos {
			retry, _ := retryRaw.(map[string]any)
			if retry == nil {
				continue
			}
			usage := retry
			if inner, ok := retry["usage"].(map[string]any); ok {
				usage = inner
			}
			responseOutput := toSafeInt64(usage["responseOutputTokens"])
			tokens, ok := normalizeAntigravityTokens(
				toSafeInt64(usage["inputTokens"]), toSafeInt64(usage["outputTokens"]),
				responseOutput, toSafeInt64(usage["cacheReadTokens"]),
				toSafeInt64(usage["cacheWriteTokens"]), toSafeInt64(usage["thinkingOutputTokens"]),
				usage["responseOutputTokens"] != nil)
			if !ok || tokens.IsEmpty() {
				continue
			}
			responseID, _ := usage["responseId"].(string)
			timestamp := parseTimestampValue(usage["createdAt"])
			if timestamp == nil {
				timestamp = parseTimestampValue(usage["timestamp"])
			}
			if timestamp == nil && responseID != "" {
				if localTimestamps == nil {
					localTimestamps = localGenerationTimestamps(home, sessionID)
				}
				if ts, ok := localTimestamps[responseID]; ok {
					timestamp = &ts
				}
			}
			if timestamp == nil {
				timestamp = createdAt
			}
			if timestamp == nil {
				continue
			}
			modelID := resolveRPCChatModel(chatModel, aliases, releases, provider.LocalDateString(*timestamp))
			rawKey := responseID
			if rawKey == "" {
				rawKey = fmt.Sprintf("%d:%s:%d", *timestamp, modelID, stepIndex)
			}
			logical := logicalAntigravityKey(sessionID, rawKey)
			id := storageAntigravityID(client, logical)
			date := provider.LocalDateString(*timestamp)
			if _, err := db.Exec(`INSERT OR REPLACE INTO session_usage (
                id, session_id, client, model_id, provider_id, timestamp, step_index,
                input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
                response_id, pricing_day, parser_version, model_enum, served_model
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
				id, sessionID, client, modelID, modelid.DetectProviderFromModel(modelID),
				*timestamp, stepIndex, tokens.Input, tokens.Output, tokens.CacheRead, tokens.CacheWrite,
				tokens.Reasoning, logical, date, antigravityParserVersion,
				stringField(chatModel, "model"), stringField(chatModel, "responseModel")); err != nil {
				return err
			}
		}
	}
	return nil
}

func localGenerationTimestamps(home, sessionID string) map[string]int64 {
	out := map[string]int64{}
	if sessionID == "" || strings.ContainsAny(sessionID, "/\\") {
		return out
	}
	for _, entry := range antigravityLocalConversationDirs(home) {
		path := filepath.Join(entry[0].(string), sessionID+".db")
		if _, err := os.Stat(path); err != nil {
			continue
		}
		db, err := openConversationDB(path)
		if err != nil {
			continue
		}
		for _, record := range readLocalConversationUsage(db) {
			if record.usage.responseID != "" && record.timestamp != nil {
				out[record.usage.responseID] = *record.timestamp
			}
		}
		db.Close()
	}
	return out
}

func collapseDesktopIDEOverlap(work map[[2]string]*antigravityWorkItem) {
	var overlapping []string
	for key := range work {
		if antigravityRuntime(key[1]) != agyIDE {
			continue
		}
		if _, ok := work[[2]string{key[0], string(agyDesktop)}]; ok {
			overlapping = append(overlapping, key[0])
		}
	}
	for _, sessionID := range overlapping {
		ide, ok := work[[2]string{sessionID, string(agyIDE)}]
		if !ok {
			continue
		}
		delete(work, [2]string{sessionID, string(agyIDE)})
		desktop, ok := work[[2]string{sessionID, string(agyDesktop)}]
		if !ok {
			continue
		}
		enrichSummary(&desktop.summary, ide.summary)
		for _, runtime := range ide.candidates {
			found := false
			for _, existing := range desktop.candidates {
				if existing == runtime {
					found = true
					break
				}
			}
			if !found {
				desktop.candidates = append(desktop.candidates, runtime)
			}
		}
	}
}

func resolveCandidateConnections(candidates []antigravityRuntime, all []antigravityConnection) []antigravityConnection {
	var out []antigravityConnection
	for _, runtime := range candidates {
		for _, connection := range all {
			if connection.Runtime != runtime {
				continue
			}
			duplicate := false
			for _, existing := range out {
				if existing.PID == connection.PID && existing.Port == connection.Port {
					duplicate = true
					break
				}
			}
			if !duplicate {
				out = append(out, connection)
			}
		}
	}
	return out
}

func enrichSummary(dest *antigravitySyncSummary, src antigravitySyncSummary) {
	dest.LastModifiedMs = newerInt64(dest.LastModifiedMs, src.LastModifiedMs)
	fillString(&dest.TrajectoryID, src.TrajectoryID)
	fillString(&dest.Title, src.Title)
	fillString(&dest.Status, src.Status)
	fillInt64(&dest.StepCount, src.StepCount)
	fillInt64(&dest.CreatedTimeMs, src.CreatedTimeMs)
	fillInt64(&dest.LastUserInputTimeMs, src.LastUserInputTimeMs)
	fillString(&dest.ProjectID, src.ProjectID)
	fillString(&dest.WorkspacePath, src.WorkspacePath)
	fillString(&dest.GitRoot, src.GitRoot)
	fillString(&dest.Repository, src.Repository)
	fillString(&dest.GitOriginURL, src.GitOriginURL)
	fillString(&dest.BranchName, src.BranchName)
	fillString(&dest.ParentConversationID, src.ParentConversationID)
	fillString(&dest.MendelExperimentIDs, src.MendelExperimentIDs)
}

func fillString(dest **string, src *string) {
	if *dest == nil {
		*dest = src
	}
}

func fillInt64(dest **int64, src *int64) {
	if *dest == nil {
		*dest = src
	}
}

func newerInt64(a, b *int64) *int64 {
	if a == nil {
		return b
	}
	if b == nil {
		return a
	}
	if *b > *a {
		return b
	}
	return a
}

func summaryFromTrajectory(item map[string]any) antigravitySyncSummary {
	var summary antigravitySyncSummary
	for _, key := range []string{"lastModifiedTime", "lastModified", "updatedAt"} {
		if ts := parseTimestampValue(item[key]); ts != nil {
			summary.LastModifiedMs = ts
			break
		}
	}
	if value, ok := item["mendelExperimentIds"].([]any); ok {
		var ids []string
		for _, id := range value {
			if s, ok := id.(string); ok {
				ids = append(ids, s)
			}
		}
		joined := strings.Join(ids, ",")
		summary.MendelExperimentIDs = &joined
	} else if s, ok := item["mendelExperimentIds"].(string); ok {
		summary.MendelExperimentIDs = &s
	}
	if s, ok := item["trajectoryId"].(string); ok {
		summary.TrajectoryID = &s
	}
	if s, ok := item["summary"].(string); ok {
		summary.Title = &s
	}
	if s, ok := item["status"].(string); ok {
		summary.Status = &s
	}
	if n, ok := item["stepCount"].(float64); ok {
		v := int64(n)
		summary.StepCount = &v
	}
	summary.CreatedTimeMs = parseTimestampValue(item["createdTime"])
	summary.LastUserInputTimeMs = parseTimestampValue(item["lastUserInputTime"])
	if s, ok := item["projectId"].(string); ok {
		summary.ProjectID = &s
	}
	if workspace, ok := item["workspace"].(map[string]any); ok {
		for _, key := range []string{"workspacePath", "path"} {
			if s, ok := workspace[key].(string); ok {
				summary.WorkspacePath = &s
				break
			}
		}
		if s, ok := workspace["gitRoot"].(string); ok {
			summary.GitRoot = &s
		}
		if s, ok := workspace["repository"].(string); ok {
			summary.Repository = &s
		}
		if s, ok := workspace["gitOriginUrl"].(string); ok {
			summary.GitOriginURL = &s
		}
		if s, ok := workspace["branchName"].(string); ok {
			summary.BranchName = &s
		}
	}
	if s, ok := item["parentConversationId"].(string); ok {
		summary.ParentConversationID = &s
	}
	return summary
}

type trajectoryEntry struct {
	key  string
	item map[string]any
}

func extractTrajectoryEntries(response map[string]any) []trajectoryEntry {
	var out []trajectoryEntry
	for _, key := range []string{"trajectorySummaries", "cascadeTrajectories"} {
		container, ok := response[key]
		if !ok {
			continue
		}
		switch v := container.(type) {
		case []any:
			for _, item := range v {
				if m, ok := item.(map[string]any); ok {
					out = append(out, trajectoryEntry{item: m})
				}
			}
			return out
		case map[string]any:
			for k, item := range v {
				if m, ok := item.(map[string]any); ok {
					out = append(out, trajectoryEntry{key: k, item: m})
				}
			}
			return out
		}
	}
	for k, item := range response {
		if k == "trajectorySummaries" || k == "cascadeTrajectories" {
			continue
		}
		if m, ok := item.(map[string]any); ok {
			out = append(out, trajectoryEntry{key: k, item: m})
		}
	}
	return out
}

func firstString(value map[string]any, keys ...string) string {
	for _, key := range keys {
		if s, ok := value[key].(string); ok && s != "" {
			return s
		}
	}
	return ""
}

func stringField(value map[string]any, key string) string {
	s, _ := value[key].(string)
	return s
}

func toSafeInt64(value any) int64 {
	var result int64
	switch v := value.(type) {
	case float64:
		result = int64(v)
	case int64:
		result = v
	case json.Number:
		result, _ = v.Int64()
	case string:
		result, _ = strconv.ParseInt(v, 10, 64)
	}
	if result < 0 {
		return 0
	}
	return result
}

func parseTimestampValue(value any) *int64 {
	var result int64
	switch v := value.(type) {
	case float64:
		result = int64(v)
	case int64:
		result = v
	case string:
		if n, err := strconv.ParseInt(v, 10, 64); err == nil {
			result = n
		} else if ts, ok := parseRFC3339Millis(v); ok {
			result = ts
		} else {
			return nil
		}
	default:
		return nil
	}
	if result <= 0 {
		return nil
	}
	return &result
}

func providerDateNow() string {
	return provider.LocalDateString(time.Now().UnixMilli())
}
