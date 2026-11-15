package main

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"
	"time"

	airplay "github.com/skrashevich/station-airplay"
)

// A receiver that plays what the test says and keeps the commands it got.
type fakeReceiver struct {
	mu       sync.Mutex
	status   airplay.Status
	commands []string
	refuse   error
}

func (f *fakeReceiver) Start(context.Context) error { return nil }
func (f *fakeReceiver) Stop(context.Context) error  { return nil }
func (f *fakeReceiver) Status() airplay.Status {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.status
}
func (f *fakeReceiver) Control(_ context.Context, action string) error {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.commands = append(f.commands, action)
	return f.refuse
}
func (f *fakeReceiver) set(connected, playing bool) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.status.Connected, f.status.Playing = connected, playing
}

// The gateway's Host API as the app sees it: the letters it got, refused while `refuse` is set.
type fakeGateway struct {
	mu      sync.Mutex
	letters []string
	refuse  bool
}

func (g *fakeGateway) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	g.mu.Lock()
	defer g.mu.Unlock()
	if r.URL.Path != "/api/v1/apps/host/call/bus.call" || r.Header.Get("Authorization") != "Bearer t0ken" {
		http.Error(w, "wrong", http.StatusNotFound)
		return
	}
	if g.refuse {
		http.Error(w, `{"error":"ждёт «да» владельца"}`, http.StatusForbidden)
		return
	}
	var letter struct {
		To, Handle string
		Input      struct{ State string }
	}
	body, _ := io.ReadAll(r.Body)
	_ = json.Unmarshal(body, &letter)
	g.letters = append(g.letters, letter.To+" "+letter.Handle+" "+letter.Input.State)
	w.Write([]byte(`{"device":"*"}`))
}

func (g *fakeGateway) got() []string {
	g.mu.Lock()
	defer g.mu.Unlock()
	return append([]string(nil), g.letters...)
}

func newApp(rx receiver, gateway string, clock *time.Time) *app {
	return &app{
		rx: rx, token: "t0ken", gateway: gateway, client: http.DefaultClient,
		logf: func(string, ...any) {}, now: func() time.Time { return *clock },
	}
}

func post(t *testing.T, handler http.Handler, path, token, body string) (int, string) {
	t.Helper()
	request := httptest.NewRequest(http.MethodPost, path, strings.NewReader(body))
	if token != "" {
		request.Header.Set("Authorization", "Bearer "+token)
	}
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, request)
	return recorder.Code, strings.TrimSpace(recorder.Body.String())
}

func TestThePlayersCommandsReachTheReceiverOnlyWithTheToken(t *testing.T) {
	rx := &fakeReceiver{}
	clock := time.Now()
	handler := newApp(rx, "", &clock).routes()

	code, _ := post(t, handler, "/call/control", "", `{"payload":{"action":"pause"}}`)
	if code != http.StatusUnauthorized {
		t.Fatalf("without the token: %d", code)
	}
	// As the gateway sends the news, and plainly.
	for _, body := range []string{`{"from":"system/player","event":"control","payload":{"action":"pause"}}`, `{"action":"resume"}`} {
		if code, answer := post(t, handler, "/call/control", "t0ken", body); code != http.StatusOK || answer != "null" {
			t.Fatalf("%s: %d %s", body, code, answer)
		}
	}
	if code, _ := post(t, handler, "/call/control", "t0ken", `{"action":"louder"}`); code != http.StatusBadRequest {
		t.Fatalf("an unknown command: %d", code)
	}
	rx.refuse = airplay.ErrNoSender
	if code, answer := post(t, handler, "/call/control", "t0ken", `{"action":"next"}`); code != http.StatusConflict || !strings.Contains(answer, "ничего не играет") {
		t.Fatalf("nothing plays: %d %s", code, answer)
	}
	if got := strings.Join(rx.commands, ","); got != "pause,resume,next" {
		t.Fatalf("commands: %s", got)
	}

	request := httptest.NewRequest(http.MethodGet, "/health", nil)
	recorder := httptest.NewRecorder()
	handler.ServeHTTP(recorder, request)
	if recorder.Code != http.StatusOK {
		t.Fatalf("health: %d", recorder.Code)
	}
	rx.set(true, true)
	code, answer := post(t, handler, "/call/status", "t0ken", "")
	if code != http.StatusOK || !strings.Contains(answer, `"state":"playing"`) {
		t.Fatalf("status: %d %s", code, answer)
	}
}

func TestTellsTheGatewayEachChangeOnceAndAgainAfterARefusal(t *testing.T) {
	gateway := &fakeGateway{refuse: true}
	server := httptest.NewServer(gateway)
	defer server.Close()
	rx := &fakeReceiver{}
	clock := time.Now()
	a := newApp(rx, server.URL, &clock)
	ctx := context.Background()

	// The owner has not said «да» yet: refused, not tried again at once.
	a.look(ctx)
	a.look(ctx)
	if len(gateway.got()) != 0 || a.told != "" {
		t.Fatalf("refused: %v, told %q", gateway.got(), a.told)
	}
	gateway.mu.Lock()
	gateway.refuse = false
	gateway.mu.Unlock()
	a.look(ctx)
	if len(gateway.got()) != 0 {
		t.Fatal("tried again before its pause")
	}
	clock = clock.Add(tellAgain + time.Second)
	a.look(ctx)
	rx.set(true, true)
	a.look(ctx)
	a.look(ctx)
	rx.set(true, false)
	a.look(ctx)
	rx.set(false, false)
	a.look(ctx)
	want := "system/player playing stopped|system/player playing playing|system/player playing paused|system/player playing stopped"
	if got := strings.Join(gateway.got(), "|"); got != want {
		t.Fatalf("letters:\n%s\nwant\n%s", got, want)
	}
}

func TestTheNamePhonesSee(t *testing.T) {
	for given, want := range map[string]string{
		"":                      "DuoWave",
		"  Кухня ":              "Кухня",
		"Кухня\nвторая":         "Кухнявторая",
		strings.Repeat("я", 40): strings.Repeat("я", 31),
	} {
		if got := receiverName(given); got != want {
			t.Errorf("%q: %q, want %q", given, got, want)
		}
	}
}

func TestTheLibrarysJournalKeepsOnlyTrouble(t *testing.T) {
	var kept strings.Builder
	logger := troubleOnly{&kept}
	for _, line := range []string{
		"RTSP SETUP /3063950120405912930\n",
		"RTSP response peer=\"192.0.2.10\" method=RECORD status=200\n",
		"codec error: unsupported audio format\n",
		"PTP: Timeout waiting for the grandmaster\n",
	} {
		if n, err := logger.Write([]byte(line)); err != nil || n != len(line) {
			t.Fatalf("%q: %d %v", line, n, err)
		}
	}
	want := "codec error: unsupported audio format\nPTP: Timeout waiting for the grandmaster\n"
	if kept.String() != want {
		t.Fatalf("kept %q", kept.String())
	}
}
