class Wassette < Formula
  desc "Wassette: A security-oriented runtime that runs WebAssembly Components via MCP"
  homepage "https://github.com/microsoft/wassette"
  # Change this to install a different version of wassette.
  # The release tag in GitHub must exist with a 'v' prefix (e.g., v0.1.0).
  version "0.6.0"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_amd64.tar.gz"
      sha256 "cafa5a0946484e6924c724cf46c1e4e7020a4d55f8061cfc655b49e4be5a7d8d"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_arm64.tar.gz"
      sha256 "e0467689e5b647764f7b8e686622cde141a0923c00eca02a594bc8171407e598"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_amd64.tar.gz"
      sha256 "1fbf2f342b07835ca9dca048bb889e27429becc6fbedbd1d296778fe38361fc9"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_arm64.tar.gz"
      sha256 "4e82f5ee3a2c1eaacf2c740da4d9daaa8bb88359743983411d392f609f9e7520"
    end
  end

  def install
    bin.install "wassette"
  end

  test do
    # Check if the installed binary's version matches the formula's version
    assert_match "wassette-mcp-server #{version}", shell_output("#{bin}/wassette --version")
  end
end
