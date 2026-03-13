class Repomap < Formula
  desc "MCP server for surgical codebase access via tree-sitter symbol indexing"
  homepage "https://github.com/christophergutierrez/repomap"
  version "0.3.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.3.0/repomap-aarch64-apple-darwin.tar.gz"
      sha256 "3536e105222c9456ee63b784e05baad781676753beb5a557c8067d9a86de7f01"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.3.0/repomap-x86_64-apple-darwin.tar.gz"
      sha256 "5ac71fa404f43795da091fe65d3de62992ea863c05c52ae13061826ddf174c91"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.3.0/repomap-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "e245f16733161655ffa6b92e2907b485785ffbb91b44deee6bd2a73fed003a13"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.3.0/repomap-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "11366bcc7658002ae9419816a9ce1ab110caceeaa560ee0ff36baacc526657fd"
    end
  end

  def install
    bin.install "repomap"
  end

  test do
    system bin/"repomap", "--version"
  end
end
