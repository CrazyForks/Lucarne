class Lucarned < Formula
  desc "Local lucarne daemon for channel adapters and agent sessions"
  homepage "https://github.com/tuchg/Lucarne"
  version "0.2.1"
  license "MIT"

  depends_on :macos

  stable do
    on_macos do
      on_arm do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.2.1/lucarned-v0.2.1-aarch64-apple-darwin.tar.gz"
        sha256 "b4cf661271e8abd9c5c80c81d4a1e6ceb38de7a19ab3b83f35d1624d9b54dbc9"
      end

      on_intel do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.2.1/lucarned-v0.2.1-x86_64-apple-darwin.tar.gz"
        sha256 "b99502ed1cbc67053c9afd3e189c7cf6a1a41da56cf38ee47eaa06aebe6a5b76"
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
      bin.install "bin/lucarned"
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
