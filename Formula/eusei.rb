# Homebrew formula para o eusei.
#
# Uso:
#   brew tap StrategicProjects/eusei https://github.com/StrategicProjects/eusei
#   brew install eusei
#
# Observação: o eusei é um serviço HTTP de servidor. Em produção (Linux),
# prefira o pacote .deb das releases. Via Homebrew ele é instalado como binário
# para execução manual/dev — configure as variáveis de ambiente antes de rodar.
class Eusei < Formula
  desc "API HTTP/JSON read-only para os Web Services do SEI"
  homepage "https://github.com/StrategicProjects/eusei"
  url "https://github.com/StrategicProjects/eusei/archive/refs/tags/v0.5.0.tar.gz"
  sha256 "5806639b58d19d997d185d578e341460faaa8161482bbfd9e50e7a78fe493c68"
  license "GPL-3.0-or-later"
  head "https://github.com/StrategicProjects/eusei.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    (share/"eusei").install ".env.example" => "eusei.env.example"
    (share/"eusei").install "deploy/eusei.nginx.conf"
  end

  def caveats
    <<~EOS
      eusei é um serviço HTTP — configure as variáveis de ambiente antes de rodar.
      Exemplo (mínimo):

        EUSEI_TOKENS=meu-token \\
        SEI_IDENTIFICACAO_SERVICO=minha-chave \\
        eusei

      Modelo completo em: #{opt_share}/eusei/eusei.env.example
      Para servidor de produção (Linux + systemd + nginx), use o pacote .deb das releases.
    EOS
  end

  test do
    # sem configuração, deve recusar subir com erro claro (exit 1)
    output = shell_output("#{bin}/eusei 2>&1", 1)
    assert_match "EUSEI_TOKENS", output
  end
end
