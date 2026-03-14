class Repomap < Formula
  desc "MCP server for surgical codebase access via tree-sitter symbol indexing"
  homepage "https://github.com/christophergutierrez/repomap"
  version "0.4.0"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.4.0/repomap-aarch64-apple-darwin.tar.gz"
      sha256 "a01d7a4571565bf8b8bc1b3ef3cb698cb19d0a33675268803046e9bd683a667b"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.4.0/repomap-x86_64-apple-darwin.tar.gz"
      sha256 "1adc75828cb429ee6b144b7db73fc24445aedf861e659366466d268258fa21b5"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.4.0/repomap-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "397308790eca9279c3a95a1b3fdcd5001d03c19e62120123f671dd338e9d10ef"
    elsif Hardware::CPU.intel?
      url "https://github.com/christophergutierrez/repomap/releases/download/v0.4.0/repomap-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "d3cbcc6509143868cc9e5f35c5dcaea7ae693371391287d3bccf436c0867c68a"
    end
  end

  def install
    bin.install "repomap"
  end

  test do
    system bin/"repomap", "--version"
  end
end
