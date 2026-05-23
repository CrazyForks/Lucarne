class Lucarned < Formula
  desc "Local lucarne daemon for channel adapters and agent sessions"
  homepage "https://github.com/tuchg/Lucarne"
  version "0.3.2"
  license "MIT"

  depends_on :macos

  stable do
    on_macos do
      on_arm do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.3.2/lucarned-v0.3.2-aarch64-apple-darwin.tar.xz"
        sha256 "1e5bb8b04af006025894fd6877b38974459afea5e8c447ca4f34f60c75d82cec"
      end

      on_intel do
        url "https://github.com/tuchg/Lucarne/releases/download/v0.3.2/lucarned-v0.3.2-x86_64-apple-darwin.tar.xz"
        sha256 "1dfa2651077bf8a67ababfb84c8d2cba953d924df8ac1fc9e09731c4b61b79b5"
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
