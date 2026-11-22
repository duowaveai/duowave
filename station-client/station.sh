#!/bin/sh
# DuoWave onto this Yandex Station from the station's own shell, in one line, with no computer:
#   wget -qO- http://get.duowave.app/station.sh | sh
# Log in to the station over SSH first (root and SSH: https://hackalice.me/), then type that line there.
#
# It looks at the station (reading only: a station, aarch64, root, room in /data and /tmp, memory), says what will
# happen, and changes nothing before «да». Then it downloads the release's station build (duowave-station-aarch64.tar.gz,
# about 8 MB) into /tmp, checks its SHA-256 and its release.json, puts the program into /data/opt/duowave/bin and runs
# the program's own `duowave station install`: its .env, the service, the way back (restore.sh, uninstall.sh), the start
# and, once it answers /health, the start with the station. The same commands the YAM package runs
# (station-client/yam). At the end it asks for the panel's password (`duowave panel setup`) and whether the station's
# voice goes to DuoWave now (`duowave station point local --reboot`: a reboot, about a minute without a voice).
#
# Plain HTTP: the station's busybox has no TLS, its wget cannot fetch https:// at all. So what comes is checked here:
# release.yml writes the release's version and the SHA-256 of its station build into this script
# (scripts/release/station-script.sh), and the catalog's site (packages.duowave.app, GitHub Pages) takes this script
# and that build only after its own build checked the build's signature with the update key. get.duowave.app is a
# Cloudflare Worker in front of it that answers plain HTTP for /station.sh and /station/ only
# (scripts/release/cloudflare): the catalog keeps HTTPS for everyone else. The checksum holds the download to this
# script; the script itself still comes unencrypted. The computer's one-line install (scripts/install) goes over HTTPS
# and checks the signature itself.
#
# Arguments (`| sh -s -- …`): --yes says «да» to all (the install, then the voice and a reboot), for a run with no
# terminal; --lang en|ru the words (else the locale's ru*/en*, else Russian).
# For a check or a mirror: DUOWAVE_BASE (where <version>/duowave-station-aarch64.tar.gz lie), DUOWAVE_VERSION and
# DUOWAVE_SHA256 (this file straight from git carries none), DUOWAVE_FILE (a build already on the station); the paths
# move with DUOWAVE_CFG, DUOWAVE_DIR, DUOWAVE_TMP, DUOWAVE_MEMINFO, DUOWAVE_TTY (station-client/tests/station-install-check.sh).
#
# Everything is inside main(), called on the last line: a download cut short runs nothing.

main() {
    set -eu
    VERSION=${DUOWAVE_VERSION:-@VERSION@}
    SHA256=${DUOWAVE_SHA256:-@SHA256@}
    BASE=${DUOWAVE_BASE:-http://get.duowave.app/station}
    NAME=duowave-station-aarch64.tar.gz
    CFG=${DUOWAVE_CFG:-/system/vendor/quasar/quasar.cfg}
    DIR=${DUOWAVE_DIR:-/data/opt/duowave}
    TMP=${DUOWAVE_TMP:-/tmp}
    MEMINFO=${DUOWAVE_MEMINFO:-/proc/meminfo}
    TTY=${DUOWAVE_TTY:-/dev/tty}
    BIN=$DIR/bin/duowave
    # What the computer's setup asks for (gateway/src/placement/moves.rs): 30 MB in /data at the first look, 20 MB of
    # memory for DuoWave. Here also the room in /tmp (memory) for the build while it is checked: about 30 MB.
    NEED_DATA_KB=30720
    NEED_MEMORY_KB=20480
    NEED_TMP_KB=30720

    yes=
    lang=
    while [ $# -gt 0 ]; do
        case $1 in
            --yes | -y) yes=1 ;;
            --lang)
                lang=${2:-}
                shift
                ;;
            --lang=*) lang=${1#--lang=} ;;
            *) ;;
        esac
        shift
    done
    case ${lang:-${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}} in
        en*) ru= ;;
        *) ru=1 ;;
    esac

    # The answers come from the terminal: through `wget … | sh` this script's stdin is the script itself.
    # Opened in a subshell first: a redirection that fails on `exec` would end the whole script.
    if [ -z "$yes" ]; then
        if (: <"$TTY") 2>/dev/null; then
            exec 3<"$TTY"
        else
            die "Нет терминала, чтобы спросить «да». Запустите команду в терминале колонки или добавьте в конец: | sh -s -- --yes" \
                "No terminal to ask for «yes». Run the command in the station's terminal, or end it with: | sh -s -- --yes"
        fi
    fi

    case $VERSION in
        @*) die "В этом скрипте нет версии выпуска. Возьмите его с выпуска: wget -qO- http://get.duowave.app/station.sh | sh" \
            "This script names no release. Take it from a release: wget -qO- http://get.duowave.app/station.sh | sh" ;;
    esac
    case $SHA256 in
        @* | '') die "В этом скрипте нет контрольной суммы сборки, проверить её нечем. Ничего не менял." \
            "This script has no checksum of the build, nothing to check it against. Nothing changed." ;;
    esac

    # The look: reading only.
    [ -f "$CFG" ] || die "Это не Яндекс Станция: нет $CFG. Ничего не менял." \
        "This is not a Yandex Station: no $CFG. Nothing changed."
    [ "$(id -u)" = 0 ] || die "Нужен root: зайдите на колонку как root." "Root is needed: log in to the station as root."
    machine=$(uname -m)
    [ "$machine" = aarch64 ] || die "Колонка с процессором $machine, а сборка DuoWave для колонки только для aarch64. Ничего не менял." \
        "The station's processor is $machine, and DuoWave's build for stations is aarch64 only. Nothing changed."

    say "Колонка: $(model), прошивка $(firmware)" "Station: $(model), firmware $(firmware)"
    voice=$(sed -n 's#.*"uniProxyUrl" *: *"\([^"]*\)".*#\1#p' "$CFG" | head -n 1)
    case $voice in
        *yandex.net*) say "Голос сейчас идёт к Яндексу." "The voice goes to Yandex now." ;;
        '') ;;
        *) say "Голос сейчас идёт на $voice" "The voice goes to $voice now." ;;
    esac

    installed=
    old_kb=0
    if [ -f "$BIN" ]; then
        if [ "$(cat "$DIR/managed-by" 2>/dev/null)" = yam ]; then
            die "DuoWave здесь поставлен пакетом YAM. Обновляйте его так же: yam install duowave/gateway. Ничего не менял." \
                "DuoWave was installed here by the YAM package. Update it the same way: yam install duowave/gateway. Nothing changed."
        fi
        installed=$("$BIN" version </dev/null 2>/dev/null | head -n 1 || true)
        old_kb=$(($(wc -c <"$BIN") / 1024))
        say "DuoWave уже стоит (${installed:-версия неизвестна}). Поставлю поверх, данные останутся." \
            "DuoWave is here already (${installed:-version unknown}). It goes over it, the data stays."
    fi

    data_kb=$(free_kb "$(dirname "$DIR")")
    if [ -n "$data_kb" ] && [ $((data_kb + old_kb)) -lt $NEED_DATA_KB ]; then
        die "В /data свободно $((data_kb / 1024)) МБ, а DuoWave нужно $((NEED_DATA_KB / 1024)) МБ. Ничего не менял." \
            "/data has $((data_kb / 1024)) MB free, and DuoWave needs $((NEED_DATA_KB / 1024)) MB. Nothing changed."
    fi
    tmp_kb=$(free_kb "$TMP")
    if [ -n "$tmp_kb" ] && [ "$tmp_kb" -lt $NEED_TMP_KB ]; then
        die "В $TMP свободно $((tmp_kb / 1024)) МБ, а на время установки нужно $((NEED_TMP_KB / 1024)) МБ. Ничего не менял." \
            "$TMP has $((tmp_kb / 1024)) MB free, and the install needs $((NEED_TMP_KB / 1024)) MB there for a while. Nothing changed."
    fi
    memory_kb=$(sed -n 's/^MemAvailable: *\([0-9]*\).*/\1/p' "$MEMINFO" 2>/dev/null | head -n 1)
    if [ -n "$memory_kb" ] && [ "$memory_kb" -lt $NEED_MEMORY_KB ]; then
        die "У колонки свободно $((memory_kb / 1024)) МБ ОЗУ, а DuoWave нужно $((NEED_MEMORY_KB / 1024)) МБ. Ничего не менял." \
            "The station has $((memory_kb / 1024)) MB of memory free, and DuoWave needs $((NEED_MEMORY_KB / 1024)) MB. Nothing changed."
    fi
    if updates_on; then
        say "Внимание: обновления от Яндекса не отключены. Обновление вернёт обычную Алису и может закрыть root. Отключите их по вики: https://hackalice.me/" \
            "Note: Yandex's updates are not turned off. An update brings plain Alice back and may close root. Turn them off as the wiki says: https://hackalice.me/"
    fi

    say "" ""
    say "Что будет:" "What happens:"
    say "  - скачаю DuoWave $VERSION для колонки (около 8 МБ) и проверю контрольную сумму," \
        "  - DuoWave $VERSION for the station is downloaded (about 8 MB) and its checksum checked,"
    say "  - программа встанет в $DIR (около 20 МБ), запустится и будет запускаться вместе с колонкой," \
        "  - the program goes into $DIR (about 20 MB), starts, and starts with the station from then on,"
    say "  - голос колонки пока останется как есть, переключить его я спрошу в конце," \
        "  - the station's voice stays as it is for now, switching it is asked at the end,"
    say "  - обратно к Яндексу: sh $DIR/restore.sh, убрать DuoWave совсем: sh $DIR/uninstall.sh" \
        "  - back to Yandex: sh $DIR/restore.sh, DuoWave off the station: sh $DIR/uninstall.sh"
    agree "Ставить DuoWave на колонку? [да/нет]" "Install DuoWave on the station? [yes/no]" || {
        say "Ничего не менял." "Nothing changed."
        return 0
    }

    work=$(mktemp -d "$TMP/duowave-station.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    trap 'rm -rf "$work"; exit 130' INT TERM
    archive=$work/$NAME
    if [ -n "${DUOWAVE_FILE:-}" ]; then
        step "Беру сборку $DUOWAVE_FILE" "Taking the build $DUOWAVE_FILE"
        cp "$DUOWAVE_FILE" "$archive" || fail "Не прочитать $DUOWAVE_FILE" "Cannot read $DUOWAVE_FILE"
    else
        step "Скачиваю DuoWave $VERSION" "Downloading DuoWave $VERSION"
        wget -q -O "$archive" "$BASE/$VERSION/$NAME" </dev/null ||
            fail "Не скачалось: $BASE/$VERSION/$NAME. Колонка видит интернет? Проверка: wget -qO- http://get.duowave.app/station.sh | head -n 1" \
                "Download failed: $BASE/$VERSION/$NAME. Does the station reach the internet? Check: wget -qO- http://get.duowave.app/station.sh | head -n 1"
    fi
    done_
    step "Проверяю контрольную сумму и версию" "Checking the checksum and the version"
    got=$(sha256sum "$archive" | cut -d' ' -f1)
    [ "$got" = "$SHA256" ] || fail "Контрольная сумма не совпала: файл повреждён или подменён. Попробуйте ещё раз." \
        "The checksum does not match: the file is damaged or replaced. Try again."
    release=$(gunzip -c "$archive" | tar -xOf - release.json 2>/dev/null || true)
    case $release in
        *'"version":"'"$VERSION"'"'*'"platform":"duowave-station-aarch64"'*) ;;
        *) fail "В сборке не та версия или не та платформа: $release" "The build names another version or platform: $release" ;;
    esac
    gunzip -c "$archive" | tar -xf - -C "$work" duowave || fail "Сборка не распаковалась." "The build did not unpack."
    rm -f "$archive"
    chmod 755 "$work/duowave"
    "$work/duowave" version </dev/null >/dev/null 2>&1 || fail "Программа из сборки не запускается на этой колонке." \
        "The build's program does not run on this station."
    new_kb=$(($(wc -c <"$work/duowave") / 1024))
    data_kb=$(free_kb "$(dirname "$DIR")")
    if [ -n "$data_kb" ] && [ $((data_kb + old_kb)) -lt $((new_kb + 10240)) ]; then
        fail "В /data свободно $((data_kb / 1024)) МБ, а программе с базой нужно $(((new_kb + 10240) / 1024)) МБ." \
            "/data has $((data_kb / 1024)) MB free, and the program with its database needs $(((new_kb + 10240) / 1024)) MB."
    fi
    done_

    step "Кладу программу в $DIR/bin ($((new_kb / 1024)) МБ)" "Putting the program into $DIR/bin ($((new_kb / 1024)) MB)"
    # A gateway already running stops first: its program's room comes back, and a running program cannot be written.
    if [ -f "$DIR/gateway.sh" ]; then setsid sh "$DIR/gateway.sh" stop </dev/null >/dev/null 2>&1 || true; fi
    mkdir -p "$DIR/bin"
    rm -f "$BIN"
    if ! cp "$work/duowave" "$BIN"; then
        rm -f "$BIN"
        # An earlier install's voice may go to the gateway just stopped: its way back, when it is there.
        if [ -f "$DIR/restore.sh" ]; then
            say "Голос колонки обратно к Яндексу: sh $DIR/restore.sh" "The station's voice back to Yandex: sh $DIR/restore.sh" >&2
        fi
        fail "Программа не записалась в $BIN. Место в /data: df -h /data" "The program was not written to $BIN. Room in /data: df -h /data"
    fi
    chmod 755 "$BIN"
    rm -rf "$work"
    sync
    done_

    step "Ставлю DuoWave и запускаю" "Installing DuoWave and starting it"
    say "" ""
    "$BIN" station install </dev/null || fail "DuoWave не запустился. Что случилось: $BIN station log" \
        "DuoWave did not start. What happened: $BIN station log"
    say "" ""

    # The panel's password, while the gateway runs and before any reboot: the program asks it itself, with no echo
    # (exit 3: the panel has one already, 2: too short or typed twice differently).
    set_password=
    if [ -z "$yes" ]; then
        if agree "Задать пароль пульта DuoWave сейчас? [да/нет]" "Set the password of DuoWave's panel now? [yes/no]"; then
            for try in 1 2 3; do
                code=0
                "$BIN" panel setup --lang "$(if [ -n "$ru" ]; then echo ru; else echo en; fi)" <&3 || code=$?
                case $code in
                    0 | 3)
                        set_password=1
                        break
                        ;;
                    2) [ "$try" = 3 ] || say "Ещё раз." "Once more." ;;
                    *) break ;;
                esac
            done
        fi
        [ -n "$set_password" ] || say "Пароль пульта зададите при первом входе. Пульт спросит код первой настройки, он показан выше. Показать снова: $BIN station code" \
            "The panel's password is set at the first visit. The panel asks for the first-run code shown above. To show it again: $BIN station code"
    fi

    say "" ""
    if agree "Направить голос колонки в DuoWave сейчас? Колонка перезагрузится, около минуты без голоса. [да/нет]" \
        "Send the station's voice to DuoWave now? The station reboots, about a minute without a voice. [yes/no]"; then
        say "Готово: DuoWave на колонке. Через минуту колонка ответит уже через него." \
            "Done: DuoWave is on the station. In a minute the station answers through it."
        # A new start, a new first-run code: the one above is gone with the reboot.
        [ -n "$set_password" ] || say "После перезагрузки код первой настройки пульта будет новый. Показать его: $BIN station code" \
            "After the reboot the panel's first-run code is a new one. To show it: $BIN station code"
        "$BIN" station point local --reboot </dev/null
    else
        say "Готово: DuoWave на колонке, голос остался как был. Направить его в DuoWave потом: $BIN station point local --reboot" \
            "Done: DuoWave is on the station, the voice stays as it was. To send it to DuoWave later: $BIN station point local --reboot"
    fi
}

say() {
    if [ -n "$ru" ]; then printf '%s\n' "$1"; else printf '%s\n' "$2"; fi
}

die() {
    say "$1" "$2" >&2
    exit 1
}

# A step: its line now, «готово» at done_; a failure says why and leaves nothing of its own in /tmp.
step() {
    if [ -n "$ru" ]; then printf '%s… ' "$1"; else printf '%s… ' "$2"; fi
}

done_() {
    say "готово" "done"
}

fail() {
    say "не вышло" "failed"
    die "$1" "$2"
}

# «да» from the terminal (fd 3), or --yes.
agree() {
    if [ -n "$yes" ]; then return 0; fi
    if [ -n "$ru" ]; then printf '%s ' "$1"; else printf '%s ' "$2"; fi
    answer=
    read -r answer <&3 || true
    case $answer in
        д* | Д* | y* | Y*) return 0 ;;
        *) return 1 ;;
    esac
}

# Free kilobytes of the file system that holds the path, as df says it; empty when it does not.
free_kb() {
    df -k "$1" 2>/dev/null | tail -n 1 | {
        read -r _ _ _ free _ || true
        case ${free:-} in '' | *[!0-9]*) ;; *) echo "$free" ;; esac
    }
}

model() {
    name=$(sed -n 's/.*"deviceType" *: *"\([^"]*\)".*/\1/p' "$CFG" | head -n 1)
    echo "${name:-?}"
}

firmware() {
    version=$(sed -n 's/.*"softwareVersion" *: *"\([^"]*\)".*/\1/p' "$CFG" | head -n 1)
    echo "${version:-?}"
}

# Yandex's updates still on: the same look as the computer's setup (gateway/src/stations/connect/station_scripts.rs).
updates_on() {
    q=$(dirname "$CFG")
    if ! grep -q -- '--updatesd' "$q/launch.json" 2>/dev/null; then return 1; fi
    if head -2 "$q/device_ota_update.sh" 2>/dev/null | grep -q 'disabled by yandex-noupdate'; then return 1; fi
    if [ -f /data/opt/backup/ota_script.pub.orig ] && ! cmp -s /data/opt/backup/ota_script.pub.orig "$q/ota_script.pub"; then
        return 1
    fi
    return 0
}

ru=1
main "$@"
