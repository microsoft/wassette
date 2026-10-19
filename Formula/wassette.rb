class Wassette < Formula
  desc "Wassette: A security-oriented runtime that runs WebAssembly Components via MCP"
  homepage "https://github.com/microsoft/wassette"
  # Change this to install a different version of wassette.
  # The release tag in GitHub must exist with a 'v' prefix (e.g., v0.1.0).
  version "0.8.1"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_amd64.tar.gz"
      sha256 "ca3d5c15f487550cba823a2b3a11871045f59d1ef1c9ce9aa93cef1521eb16b2"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_arm64.tar.gz"
      sha256 "45917de0a6be3208aad4e3e1594668f22eaa185d671863dfa8f51fc8f3901223"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_amd64.tar.gz"
      sha256 "2a421a73c700798076507df27fbbb25ce4d35862c6b09c4a3fd7297e00c45736"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_arm64.tar.gz"
      sha256 "4081cfcff1de59e4e57455eb813b7a4e831133185f58e16fee14dea44e3a8731"
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
