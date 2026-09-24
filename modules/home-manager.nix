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
        List of feeds written to `feeds.toml`. Leave empty to manage feeds
        imperatively (e.g. via `shinbun import`), since a declarative
        `feeds.toml` is read-only and cannot be modified by the app.
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

    xdg.configFile."shinbun/feeds.toml" = mkIf (cfg.feeds != [ ]) {
      source = tomlFormat.generate "feeds.toml" { feeds = cleanFeeds; };
    };

    xdg.configFile."shinbun/config.toml" = mkIf (cfg.settings != { }) {
      source = tomlFormat.generate "config.toml" cfg.settings;
    };
  };
}
