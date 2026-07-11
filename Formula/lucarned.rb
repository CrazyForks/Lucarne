class Lucarned < Formula
  desc "Local lucarne daemon for channel adapters and agent sessions"
  homepage "https://github.com/tuchg/Lucarne"
  version "0.5.0"
  license "MIT"

  depends_on :macos

  stable do
    on_macos do
      on_arm do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.5.0/lucarned-v0.5.0-aarch64-apple-darwin.tar.xz"
        sha256 "3f97b8f150d2b72eca6e22b016ce819b1dbc6e6b623c987f851372a7dff1f2ba"
      end

      on_intel do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.5.0/lucarned-v0.5.0-x86_64-apple-darwin.tar.xz"
        sha256 "a5d39d4b17ce9e9a7a3e06875c239fff32fe99e37db9a9a76feab89541d08ca0"
      end
    end
  end

  head do
    url "https://github.com/tuchg/Lucarne.git", branch: "main"

    depends_on "pkg-config" => :build
    depends_on "rust" => :build
    depends_on "openssl@3"
  end

  def install
    if build.head?
      ENV["OPENSSL_DIR"] = Formula["openssl@3"].opt_prefix

      system "cargo", "install", "--path", "crates/lucarned", "--root", prefix, "--no-track"
    else
      bin.install "lucarned"
    end
  end

  service do
    run [opt_bin/"lucarned"]
    environment_variables PATH: ENV.fetch("HOMEBREW_PATH", std_service_path_env)
    keep_alive false
    log_path var/"log/lucarned/brew.out.log"
    error_log_path var/"log/lucarned/brew.err.log"
    working_dir HOMEBREW_PREFIX
  end

  def caveats
    <<~EOS
      lucarned creates ~/.lucarned/lucarned.yaml during setup.

      Basic setup:
        lucarned init
        brew services start lucarned

      lucarned init is interactive; run it in a terminal. It can validate
      Telegram settings and show a WeChat QR code login.

      Config can enable selected agents (omit agents to enable all compiled agents):
        agents:
          - codex
          - pi

      Config can enable channels before starting service, for example Telegram:
        channels:
          telegram:
            enabled: true
            token: "123456:REDACTED"
            entry_chat_id: 123456789

      Common commands:
        brew services start lucarned
        brew services stop lucarned
        brew services restart lucarned

      Logs:   ~/.lucarned/logs/lucarned.YYYY-MM-DD.log
      Config: ~/.lucarned/lucarned.yaml
      State:  ~/.lucarned/state.sqlite3
      Brew service logs:
        #{var}/log/lucarned/brew.out.log
        #{var}/log/lucarned/brew.err.log
    EOS
  end

  test do
    ENV["HOME"] = testpath
    ENV.delete "LUCARNE_CONFIG"
    ENV.delete "LUCARNED_CONFIG"
    ENV.delete "LUCARNE_STATE_DB"
    ENV.delete "LUCARNE_LOG_FILE"

    system bin/"lucarned"
    config = testpath/".lucarned/lucarned.yaml"
    assert_path_exists config
    assert_match "agents:", config.read
    assert_match "telegram:", config.read
    assert_match "wechat:", config.read
    assert_match "enabled: false", config.read
  end
end
