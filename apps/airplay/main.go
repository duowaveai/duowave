// duowave-airplay: the AirPlay receiver of a Yandex station as a DuoWave app (docs/plans/media-apps.md, stage 2).
//
// The receiver is github.com/skrashevich/station-airplay (Apache-2.0) embedded as a library; this program is what the
// gateway's supervisor runs (runtime: process, docs/plugins/AUTHORING.md, «Приложения»):
//
//   - it listens on 127.0.0.1:$PORT: GET /health; POST /call/control — the player's command a person said
//     («Алиса, пауза»: the news `control` of `system/player`, {payload: {action}}); POST /call/status — what it does;
//   - it tells the gateway when a phone starts or stops playing (`system/player` → `playing` through the Host API):
//     the gateway pauses Alice's player then and sends the voice commands here;
//   - its pairing lives in $DUOWAVE_APP_DATA/airplay; phones see it as $DUOWAVE_STATION_NAME;
//   - SIGTERM stops the receiver within the supervisor's three seconds: the sound card's mixer and port 7000 are let go.
//
// It has no voice of its own: the library has none (the author's command line listens to Alice's IPC, this does not),
// so a «пауза» is never caught twice.
package main

import (
	"context"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"
	"unicode/utf8"

	airplay "github.com/skrashevich/station-airplay"
)

var version = "dev"

const (
	// The name phones see when the gateway gives none.
	defaultName = "DuoWave"
	// How often the receiver's state is looked at.
	lookEvery = 500 * time.Millisecond
	// A start that failed (port 7000 taken) is tried again this often.
	startAgain = 30 * time.Second
	// A letter the gateway refused (the owner has not said «да» yet) is sent again after this, then twice as long.
	tellAgain    = 15 * time.Second
	tellAgainMax = 5 * time.Minute
	// A command waits for the phone this long: the gateway gives a voice call two seconds.
	controlLimit = 1800 * time.Millisecond
	// What SIGTERM leaves for the receiver to let go: the supervisor kills after three seconds.
	stopLimit = 2500 * time.Millisecond
)

// The receiver library's own log lines that reach the gateway's journal: only what went wrong. It writes tens of
// lines on every connection (each RTSP request and answer), and the gateway drops the rest of a burst after a few —
// the journal filled with RTSP and lost the lines that mattered (checked on the station, 15.11).
var troubleWords = []string{"error", "fail", "warn", "panic", "cannot", "can't", "refus", "denied", "invalid", "timeout", "lost", "unsupported"}

// A writer that passes on only the lines with a word of trouble.
type troubleOnly struct{ to io.Writer }

func (t troubleOnly) Write(line []byte) (int, error) {
	lower := strings.ToLower(string(line))
	for _, word := range troubleWords {
		if strings.Contains(lower, word) {
			return t.to.Write(line)
		}
	}
	return len(line), nil
}

// The player's commands a person may say.
var actions = map[string]bool{"pause": true, "resume": true, "next": true, "previous": true, "stop": true}

// What of the receiver the app uses: the library's Receiver, a stand-in in the tests.
type receiver interface {
	Start(context.Context) error
	Stop(context.Context) error
	Status() airplay.Status
	Control(context.Context, string) error
}

type app struct {
	rx      receiver
	token   string
	gateway string
	client  *http.Client
	logf    func(string, ...any)
	now     func() time.Time

	mu       sync.Mutex
	startErr string
	// What the gateway was told last ("" — nothing yet), and when to try again after a refusal.
	told      string
	nextTell  time.Time
	tellPause time.Duration
}

// The receiver's state in the gateway's words: playing, paused (a sender holds the session), stopped.
func stateOf(status airplay.Status) string {
	switch {
	case status.Connected && status.Playing:
		return "playing"
	case status.Connected:
		return "paused"
	default:
		return "stopped"
	}
}

// A name of at most 63 bytes, as AirPlay takes it, cut on a letter's boundary; control characters out.
func receiverName(name string) string {
	name = strings.TrimSpace(strings.Map(func(r rune) rune {
		if r < 0x20 || r == 0x7f {
			return -1
		}
		return r
	}, name))
	if name == "" {
		return defaultName
	}
	for len(name) > 63 {
		_, size := utf8.DecodeLastRuneInString(name)
		name = name[:len(name)-size]
	}
	return strings.TrimSpace(name)
}

func (a *app) authorized(r *http.Request) bool {
	return a.token == "" || r.Header.Get("Authorization") == "Bearer "+a.token
}

func writeJSON(w http.ResponseWriter, code int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(value)
}

func (a *app) routes() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /health", func(w http.ResponseWriter, r *http.Request) {
		writeJSON(w, http.StatusOK, map[string]bool{"ok": true})
	})
	mux.HandleFunc("POST /call/control", a.control)
	mux.HandleFunc("POST /call/status", a.status)
	return mux
}

// The player's command: {payload: {action}} as the gateway sends the news, or {action}.
func (a *app) control(w http.ResponseWriter, r *http.Request) {
	if !a.authorized(r) {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "no token"})
		return
	}
	var input struct {
		Action  string `json:"action"`
		Payload struct {
			Action string `json:"action"`
		} `json:"payload"`
	}
	body, _ := io.ReadAll(io.LimitReader(r.Body, 64*1024))
	_ = json.Unmarshal(body, &input)
	action := input.Payload.Action
	if action == "" {
		action = input.Action
	}
	if !actions[action] {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "action: pause, resume, next, previous или stop"})
		return
	}
	ctx, cancel := context.WithTimeout(r.Context(), controlLimit)
	defer cancel()
	err := a.rx.Control(ctx, action)
	switch {
	case err == nil:
		a.logf("%s: done", action)
		writeJSON(w, http.StatusOK, nil)
	case errors.Is(err, airplay.ErrNoSender), errors.Is(err, airplay.ErrNotRunning):
		writeJSON(w, http.StatusConflict, map[string]string{"error": "сейчас ничего не играет по AirPlay"})
	default:
		a.logf("%s: %v", action, err)
		writeJSON(w, http.StatusBadGateway, map[string]string{"error": err.Error()})
	}
}

func (a *app) status(w http.ResponseWriter, r *http.Request) {
	if !a.authorized(r) {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "no token"})
		return
	}
	status := a.rx.Status()
	a.mu.Lock()
	startErr := a.startErr
	a.mu.Unlock()
	if status.Error == "" {
		status.Error = startErr
	}
	writeJSON(w, http.StatusOK, map[string]any{
		"state":     stateOf(status),
		"receiver":  string(status.State),
		"connected": status.Connected,
		"playing":   status.Playing,
		"name":      status.Name,
		"error":     status.Error,
		"version":   version,
	})
}

// Tells the gateway what plays (`system/player` → `playing`), once per change; a refusal is tried again later.
func (a *app) look(ctx context.Context) {
	state := stateOf(a.rx.Status())
	a.mu.Lock()
	if state == a.told || a.now().Before(a.nextTell) || a.gateway == "" {
		a.mu.Unlock()
		return
	}
	a.mu.Unlock()
	err := a.tell(ctx, state)
	a.mu.Lock()
	defer a.mu.Unlock()
	if err == nil {
		a.told, a.tellPause, a.nextTell = state, 0, time.Time{}
		a.logf("the gateway knows: %s", state)
		return
	}
	a.tellPause = min(max(a.tellPause*2, tellAgain), tellAgainMax)
	a.nextTell = a.now().Add(a.tellPause)
	a.logf("the gateway did not take «%s»: %v — again in %s", state, err, a.tellPause)
}

func (a *app) tell(ctx context.Context, state string) error {
	letter, _ := json.Marshal(map[string]any{
		"to": "system/player", "handle": "playing", "input": map[string]string{"state": state},
	})
	ctx, cancel := context.WithTimeout(ctx, 5*time.Second)
	defer cancel()
	url := strings.TrimRight(a.gateway, "/") + "/api/v1/apps/host/call/bus.call"
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, url, strings.NewReader(string(letter)))
	if err != nil {
		return err
	}
	request.Header.Set("Content-Type", "application/json")
	request.Header.Set("Authorization", "Bearer "+a.token)
	response, err := a.client.Do(request)
	if err != nil {
		return err
	}
	defer response.Body.Close()
	answer, _ := io.ReadAll(io.LimitReader(response.Body, 4096))
	if response.StatusCode/100 != 2 {
		return fmt.Errorf("HTTP %d %s", response.StatusCode, strings.TrimSpace(string(answer)))
	}
	return nil
}

// Starts the receiver, again every startAgain while it cannot (port 7000 taken by another receiver).
func (a *app) keepStarted(ctx context.Context) {
	for {
		err := a.rx.Start(ctx)
		a.mu.Lock()
		a.startErr = ""
		if err != nil {
			a.startErr = err.Error()
		}
		a.mu.Unlock()
		if err == nil {
			a.logf("AirPlay is on: %s", a.rx.Status().Name)
			return
		}
		if ctx.Err() != nil {
			return
		}
		log.Printf("AirPlay не включился: %v. Если занят порт 7000 — на колонке работает другой приёмник AirPlay "+
			"(пакет YAM skrashevich/station-airplay, AirPlay в quasar-bridge): выключите его. Ещё раз через %s.", err, startAgain)
		select {
		case <-ctx.Done():
			return
		case <-time.After(startAgain):
		}
	}
}

func (a *app) watch(ctx context.Context) {
	ticks := time.NewTicker(lookEvery)
	defer ticks.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticks.C:
			a.look(ctx)
		}
	}
}

func main() {
	log.SetFlags(0)
	log.SetOutput(os.Stderr)
	check := flag.Bool("check", false, "Check the receiver's prerequisites (network, the PCM output) and exit")
	showVersion := flag.Bool("version", false, "Print the version and exit")
	flag.Parse()
	if *showVersion {
		fmt.Printf("duowave-airplay %s\n", version)
		return
	}
	if err := run(*check); err != nil {
		log.Print(err)
		os.Exit(1)
	}
}

func run(check bool) error {
	data := os.Getenv("DUOWAVE_APP_DATA")
	if data == "" {
		return errors.New("DUOWAVE_APP_DATA: нет папки данных — программу запускает шлюз DuoWave")
	}
	info := log.New(os.Stdout, "", 0)
	rx, err := airplay.New(airplay.Config{
		Name:    receiverName(os.Getenv("DUOWAVE_STATION_NAME")),
		DataDir: filepath.Join(data, "airplay"),
		Logger:  log.New(troubleOnly{os.Stderr}, "", 0),
	})
	if err != nil {
		return err
	}
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer cancel()
	if check {
		result, err := rx.Check(ctx)
		if err != nil {
			return err
		}
		fmt.Printf("version=%s interface=%s ip=%s pcm=%s data=%s\n", version, result.Interface, result.IP, result.Device, result.DataDir)
		return nil
	}
	port := os.Getenv("PORT")
	if port == "" {
		return errors.New("PORT: нет порта — программу запускает шлюз DuoWave")
	}
	a := &app{
		rx:      rx,
		token:   os.Getenv("DUOWAVE_APP_TOKEN"),
		gateway: os.Getenv("DUOWAVE_URL"),
		client:  &http.Client{Timeout: 10 * time.Second},
		logf:    info.Printf,
		now:     time.Now,
	}
	listener, err := net.Listen("tcp", net.JoinHostPort("127.0.0.1", port))
	if err != nil {
		return err
	}
	server := &http.Server{Handler: a.routes(), ReadHeaderTimeout: 5 * time.Second}
	go func() {
		if err := server.Serve(listener); err != nil && !errors.Is(err, http.ErrServerClosed) {
			log.Printf("HTTP: %v", err)
			cancel()
		}
	}()
	info.Printf("duowave-airplay %s listens on %s", version, listener.Addr())
	go a.keepStarted(ctx)
	go a.watch(ctx)
	<-ctx.Done()

	// Asked to stop: the receiver lets go of the mixer and its ports before the supervisor's three seconds are out.
	stop, done := context.WithTimeout(context.Background(), stopLimit)
	defer done()
	if err := rx.Stop(stop); err != nil {
		log.Printf("AirPlay не выключился вовремя: %v", err)
	}
	_ = server.Shutdown(stop)
	info.Printf("stopped")
	return nil
}
