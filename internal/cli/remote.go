package cli

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	qrcode "github.com/skip2/go-qrcode"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/paths"
	"github.com/fangwangme/tokenpulse/internal/remote"
	"github.com/fangwangme/tokenpulse/internal/service"
	"github.com/fangwangme/tokenpulse/internal/version"
)

func runServe(args []string) error {
	flags := flag.NewFlagSet("serve", flag.ContinueOnError)
	var port, httpsPort int
	var noTailscale, lan, quiet bool
	var webDir string
	flags.IntVar(&port, "port", 0, "local HTTP port (default 4120)")
	flags.IntVar(&httpsPort, "https-port", 0, "Tailscale Serve HTTPS port (default 8444)")
	flags.BoolVar(&noTailscale, "no-tailscale", false, "listen on localhost only, without Tailscale")
	flags.BoolVar(&lan, "lan", false, "listen on the LAN over unencrypted HTTP")
	flags.StringVar(&webDir, "web-dir", "", "serve the web app from this directory instead of the embedded build")
	flags.BoolVar(&quiet, "quiet", false, "suppress the startup banner and QR code")
	if err := flags.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return nil
		}
		return err
	}
	manager, err := config.New()
	if err != nil {
		return err
	}
	if !quiet {
		if cfg, err := manager.Load(); err == nil && cfg.Display.AutoRefreshSecs == 0 {
			fmt.Println("display.auto_refresh_secs is 0; serve refreshes every 300s")
		}
	}
	server, err := remote.NewServer(remote.Options{
		Port: port, HTTPSPort: httpsPort, NoTailscale: noTailscale, LAN: lan, WebDir: webDir, Quiet: quiet,
	})
	if err != nil {
		return err
	}
	return server.Run()
}

func runRemote(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: tokenpulse remote reset-token | url | status")
	}
	switch args[0] {
	case "-h", "--help", "help":
		fmt.Println(`Usage: tokenpulse remote <reset-token|url|status>

  reset-token  Replace the remote token and invalidate every phone session
  url          Print the pairing URL and QR code
  status       Show whether serve is running and when it last scanned`)
		return nil
	}
	dataDir, err := paths.DataDir()
	if err != nil {
		return err
	}
	state := remote.NewStateStore(dataDir)
	switch args[0] {
	case "reset-token":
		token, err := state.ResetToken()
		if err != nil {
			return err
		}
		fmt.Println("Remote token reset; every existing session is invalid.")
		base := readServeURL(dataDir)
		fmt.Printf("Pairing URL: %s\n", pairingURL(base, token))
		return nil
	case "url":
		token, err := state.Token()
		if err != nil {
			return err
		}
		base := readServeURL(dataDir)
		url := pairingURL(base, token)
		fmt.Println(url)
		if code, err := qrcode.New(url, qrcode.Medium); err == nil {
			fmt.Println(code.ToSmallString(false))
		}
		return nil
	case "status":
		client := remote.NewLocalClient(dataDir)
		if state, err := client.State(); err == nil {
			fmt.Printf("serve running: %s\n", state.Version)
			fmt.Printf("scannedAt: %s\n", displayEpoch(state.ScannedAt))
		} else {
			fmt.Println("serve not running")
		}
		return nil
	default:
		return fmt.Errorf("unknown remote command %q", args[0])
	}
}

func displayEpoch(milliseconds int64) string {
	if milliseconds <= 0 {
		return "never"
	}
	moment := time.UnixMilli(milliseconds).Local()
	return fmt.Sprintf("%s (%s ago)", moment.Format("2006-01-02 15:04:05"), shortAgo(time.Since(moment)))
}

func shortAgo(duration time.Duration) string {
	if duration < time.Minute {
		return "just now"
	}
	if duration < time.Hour {
		return fmt.Sprintf("%dm", int(duration.Minutes()))
	}
	if duration < 24*time.Hour {
		return fmt.Sprintf("%dh", int(duration.Hours()))
	}
	return fmt.Sprintf("%dd", int(duration.Hours()/24))
}

func readServeURL(dataDir string) string {
	if content, err := os.ReadFile(filepath.Join(dataDir, "serve.url")); err == nil {
		if url := strings.TrimSpace(string(content)); url != "" {
			return url
		}
	}
	port := 4120
	if version.IsTest() {
		port = 14120
	}
	return fmt.Sprintf("http://localhost:%d/", port)
}

func pairingURL(base, token string) string {
	return strings.TrimSuffix(base, "/") + "/#token=" + token
}

func runService(args []string) error {
	if len(args) == 0 {
		return errors.New("usage: tokenpulse service install | uninstall | status")
	}
	switch args[0] {
	case "-h", "--help", "help":
		fmt.Println(`Usage: tokenpulse service <install|uninstall|status>

  install    Write and start the systemd user unit or launchd agent
  uninstall  Stop and remove it
  status     Show whether it and serve are running`)
		return nil
	case "install":
		if err := service.Install(); err != nil {
			return err
		}
		fmt.Println("Service installed and started.")
		return nil
	case "uninstall":
		if err := service.Uninstall(); err != nil {
			return err
		}
		fmt.Println("Service stopped and removed.")
		return nil
	case "status":
		return service.Status()
	default:
		return fmt.Errorf("unknown service command %q", args[0])
	}
}
