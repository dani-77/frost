PREFIX ?= /usr
DESTDIR ?=
CARGO ?= cargo

OS_FAMILY := $(shell . /etc/os-release 2>/dev/null && printf '%s %s' "$$ID" "$$ID_LIKE")
ifneq ($(findstring chimera,$(OS_FAMILY)),)
PAM_VARIANT ?= chimera
else ifneq ($(findstring arch,$(OS_FAMILY)),)
PAM_VARIANT ?= arch
else ifneq ($(findstring debian,$(OS_FAMILY)),)
PAM_VARIANT ?= debian
else ifneq ($(findstring void,$(OS_FAMILY)),)
PAM_VARIANT ?= void
else
PAM_VARIANT ?= unknown
endif

BINDIR := $(DESTDIR)$(PREFIX)/bin
SHAREDIR := $(DESTDIR)$(PREFIX)/share/frost
LICENSEDIR := $(DESTDIR)$(PREFIX)/share/licenses/frost
PAMDIR := $(DESTDIR)/etc/pam.d
PAM_SOURCE := pam.d/frost-locker.$(PAM_VARIANT)

RUST_SOURCES := $(wildcard src/*.rs src/*/*.rs)
EMBEDDED_ASSETS := $(wildcard assets/*)
LICENSE_FILES := $(wildcard licenses/*.txt)
LICENSE_TARGETS := $(addprefix $(LICENSEDIR)/,$(notdir $(LICENSE_FILES)))

.PHONY: all build install uninstall clean

all: build

build: target/release/frost

target/release/frost: Cargo.toml Cargo.lock $(RUST_SOURCES) $(EMBEDDED_ASSETS)
	$(CARGO) build --release --locked

install: build
	test -f "$(PAM_SOURCE)" || { echo "Unknown PAM_VARIANT: $(PAM_VARIANT)" >&2; exit 1; }
	install -Dm755 target/release/frost "$(BINDIR)/frost"
	install -Dm644 "$(PAM_SOURCE)" "$(PAMDIR)/frost-locker"
	install -Dm644 config.example.toml "$(SHAREDIR)/config.example.toml"
	install -Dm644 LICENSE "$(LICENSEDIR)/LICENSE"
	install -m644 $(LICENSE_FILES) "$(LICENSEDIR)/"

uninstall:
	rm -f "$(BINDIR)/frost" "$(PAMDIR)/frost-locker" \
		"$(SHAREDIR)/config.example.toml" "$(LICENSEDIR)/LICENSE" \
		$(LICENSE_TARGETS)

clean:
	$(CARGO) clean
