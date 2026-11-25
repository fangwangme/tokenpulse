package cli

import (
	"errors"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"strings"

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
	var noTailscale, lan bool
	var webDir string
	flags.IntVar(&port, "port", 0, "")
	flags.IntVar(&httpsPort, "https-port", 0, "")
	flags.BoolVar(&noTailscale, "no-tailscale", false, "")
	flags.BoolVar(&lan, "lan", false, "")
	flags.StringVar(&webDir, "web-dir", "", "")
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
	if cfg, err := manager.Load(); err == nil && cfg.Display.AutoRefreshSecs == 0 {
		fmt.Println("display.auto_refresh_secs is 0; serve refreshes every 300s")
	}
	server, err := remote.NewServer(remote.Options{
		Port: port, HTTPSPort: httpsPort, NoTailscale: noTailscale, LAN: lan, WebDir: webDir,
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
			fmt.Printf("scannedAt: %d\n", state.ScannedAt)
		} else {
			fmt.Println("serve not running")
		}
		return nil
	default:
		return fmt.Errorf("unknown remote command %q", args[0])
	}
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
