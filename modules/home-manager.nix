{ self, ... }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  inherit (lib) mkIf mkOption types;

  cfg = config.programs.shinbun;

  tomlFormat = pkgs.formats.toml { };

  # Strip null-valued attrs from each feed entry: unset `nullOr` options
  # (name/tags/refresh) come through as `null`, which TOML cannot encode.
  cleanFeeds = map (f: lib.filterAttrs (_: v: v != null) f) cfg.feeds;

  feedsToml = tomlFormat.generate "feeds.toml" { feeds = cleanFeeds; };
  configToml = tomlFormat.generate "config.toml" cfg.settings;

  jq = lib.getExe pkgs.jq;
  yj = lib.getExe pkgs.yj;

  configDir = "${config.xdg.configHome}/shinbun";
  # Links declared by the previous generation, used to prune feeds that were
  # removed from the Nix config without touching ones added in-app.
  declaredFeedsState = "${config.xdg.stateHome}/shinbun/hm-declared-feeds.json";

  # Replace a leftover store symlink (from the immutable mode) with nothing, so
  # the merge below starts from a real file or from scratch.
  unlinkStale = target: ''
    if [ -L "${target}" ]; then
      run rm "${target}"
    fi
  '';

  # Deep-merge the declared settings over the existing config.toml: declared
  # keys win, keys only present in the file survive. Arrays are replaced whole.
  mergeSettingsScript = ''
    target="${configDir}/config.toml"
    ${unlinkStale "$target"}
    run mkdir -p "${configDir}"
    if [ ! -e "$target" ]; then
      run install -m644 ${configToml} "$target"
    else
      tmp="$(mktemp -d)"
      if ${yj} -tj < "$target" > "$tmp/current.json"; then
        ${yj} -tj < ${configToml} > "$tmp/nix.json"
        ${jq} -s '.[0] * .[1]' "$tmp/current.json" "$tmp/nix.json" \
          | ${yj} -jt > "$tmp/merged.toml"
        run install -m644 "$tmp/merged.toml" "$target"
      else
        warnEcho "shinbun: could not parse $target, leaving it untouched"
      fi
      rm -rf "$tmp"
    fi
  '';

  # Merge declared feeds into feeds.toml keyed by `link`: declared feeds
  # replace their existing entry in place (or are appended), feeds added
  # in-app are kept, and feeds declared last generation but not now are pruned.
  mergeFeedsScript = ''
    target="${configDir}/feeds.toml"
    state="${declaredFeedsState}"
    ${unlinkStale "$target"}
    tmp="$(mktemp -d)"
    ${yj} -tj < ${feedsToml} > "$tmp/nix.json"
    if [ -f "$state" ]; then cp "$state" "$tmp/prev.json"; else echo '[]' > "$tmp/prev.json"; fi

    ok=1
    if [ -e "$target" ]; then
      ${yj} -tj < "$target" > "$tmp/current.json" || ok=
    elif [ ${toString (builtins.length cleanFeeds)} -gt 0 ]; then
      echo '{}' > "$tmp/current.json"
    else
      # Nothing declared and no file yet: leave feeds.toml for the app to create.
      ok=
    fi

    if [ -n "$ok" ]; then
      ${jq} -n \
        --slurpfile cur "$tmp/current.json" \
        --slurpfile nix "$tmp/nix.json" \
        --slurpfile prev "$tmp/prev.json" '
          ($nix[0].feeds // []) as $n
          | ($n | map({ key: .link, value: . }) | from_entries) as $byLink
          | ($prev[0] - ($n | map(.link))) as $gone
          | ($cur[0].feeds // [])
          | map(select(.link | IN($gone[]) | not))
          | map($byLink[.link] // .)
          | (map(.link)) as $have
          | { feeds: (. + ($n | map(select(.link | IN($have[]) | not)))) }
        ' | ${yj} -jt > "$tmp/merged.toml"
      run mkdir -p "${configDir}"
      run install -m644 "$tmp/merged.toml" "$target"
    elif [ -e "$target" ]; then
      warnEcho "shinbun: could not parse $target, leaving it untouched"
    fi

    if [ -n "$ok" ] || [ ! -e "$target" ]; then
      ${jq} '[.feeds[]?.link]' "$tmp/nix.json" > "$tmp/declared.json"
      run mkdir -p "$(dirname "$state")"
      run install -m644 "$tmp/declared.json" "$state"
    fi
    rm -rf "$tmp"
  '';
in
{
  options.programs.shinbun = {
    enable = lib.mkEnableOption "the shinbun terminal RSS/Atom feed reader";

    package = lib.mkOption {
      type = types.nullOr types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "shinbun.packages.\${pkgs.stdenv.hostPlatform.system}.default";
      description = "The shinbun package to install. Set to null to skip installing a package.";
    };

    feeds = mkOption {
      type = types.listOf (
        types.submodule {
          options = {
            link = mkOption {
              type = types.str;
              example = "https://example.com/feed.xml";
              description = "Feed URL.";
            };

            name = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Display name for the feed.";
            };

            tags = mkOption {
              type = types.nullOr (types.listOf types.str);
              default = null;
              example = [ "news" "tech" ];
              description = "Tags used for grouping/filtering the feed.";
            };

            refresh = mkOption {
              type = types.nullOr types.str;
              default = null;
              example = "1h";
              description = ''
                Refresh interval, e.g. "1h", "3d", "1w". When set, the feed is
                re-fetched at startup if the interval has elapsed since the
                last successful fetch.
              '';
            };
          };
        }
      );
      default = [ ];
      description = ''
        List of feeds written to `feeds.toml`. With {option}`mutableFeeds`
        (the default) they are merged into the existing file; otherwise the
        file is a read-only symlink that the app cannot modify.
      '';
    };

    mutableFeeds = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Whether `feeds.toml` stays writable by shinbun (adding, editing and
        removing feeds in-app, `shinbun import`). On activation, declared
        feeds are merged in by `link`: each one replaces its existing entry
        or is appended, feeds added outside Nix are kept, and feeds removed
        from {option}`feeds` since the last activation are deleted. A
        declared feed deleted in-app comes back on the next activation.
        Comments in the file are not preserved.

        When disabled, `feeds.toml` is a read-only symlink to the Nix store.
      '';
    };

    mutableSettings = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Whether `config.toml` stays a regular, editable file. On activation,
        {option}`settings` is deep-merged over the existing file: declared
        keys win, other keys are kept, and arrays (e.g. `queries`) are
        replaced whole. Comments in the file are not preserved.

        When disabled, `config.toml` is a read-only symlink to the Nix store.
      '';
    };

    settings = mkOption {
      type = tomlFormat.type;
      default = { };
      example = lib.literalExpression ''
        {
          general.browser = "firefox";
          ui.show_images = true;
          ui.theme.border = "#ff0000";
          queries = [
            { name = "Unread"; query = "*"; }
          ];
        }
      '';
      description = ''
        Configuration written verbatim to `config.toml` (TOML), corresponding
        to the `[general]`, `[ui]` (including `[ui.theme]`), and `[[queries]]`
        sections. See shinbun's `src/config.rs` and `src/theme.rs` for the
        full set of supported keys.
      '';
    };
  };

  config = mkIf cfg.enable {
    home.packages = lib.mkIf (cfg.package != null) [ cfg.package ];

    xdg.configFile."shinbun/feeds.toml" = mkIf (!cfg.mutableFeeds && cfg.feeds != [ ]) {
      source = feedsToml;
    };

    xdg.configFile."shinbun/config.toml" = mkIf (!cfg.mutableSettings && cfg.settings != { }) {
      source = configToml;
    };

    # Run after linkGeneration so links from an immutable generation are
    # already cleaned up before we write real files in their place.
    home.activation.shinbunFeeds = mkIf cfg.mutableFeeds (
      lib.hm.dag.entryAfter [ "linkGeneration" ] mergeFeedsScript
    );

    home.activation.shinbunSettings = mkIf (cfg.mutableSettings && cfg.settings != { }) (
      lib.hm.dag.entryAfter [ "linkGeneration" ] mergeSettingsScript
    );
  };
}
