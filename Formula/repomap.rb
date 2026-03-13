class Repomap < Formula
  desc "MCP server for surgical codebase access via tree-sitter symbol indexing"
  homepage "https://github.com/christophergutierrez/repomap"
  version "0.3.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v#{version}/repomap-aarch64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v#{version}/repomap-x86_64-apple-darwin.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v#{version}/repomap-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v#{version}/repomap-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
    end
  end

  def install
    bin.install "repomap"
  end

  test do
    system bin/"repomap", "--version"
  end
end
