class Wassette < Formula
  desc "Wassette: A security-oriented runtime that runs WebAssembly Components via MCP"
  homepage "https://github.com/microsoft/wassette"
  # Change this to install a different version of wassette.
  # The release tag in GitHub must exist with a 'v' prefix (e.g., v0.1.0).
  version "0.3.0"

  on_macos do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_amd64.tar.gz"
      sha256 "2800d0ea1f722fb6814caed07f7b93ab1fa637f75b3a096d8b482158c8c2eb98"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_darwin_arm64.tar.gz"
      sha256 "394ede6bd2ad8420e59d4cd88e6a6c0299e6b7997b044599282f0d6941c5d575"
    end
  end

  on_linux do
    if Hardware::CPU.intel?
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_amd64.tar.gz"
      sha256 "7510424ce5ef7db02ae8e02d8dbf49ce1a723cec61451201d3107a405f56d6c9"
    else
      url "https://github.com/microsoft/wassette/releases/download/v#{version}/wassette_#{version}_linux_arm64.tar.gz"
      sha256 "fa92161cf0bf1644f368ab4f87bdcd4483079d679ed7baeebfbbeb5a0e43df9a"
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
