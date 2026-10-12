{
  description = "Rust CLI and MCP controller for isolated Tor Browser sessions";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      runtime = [
        pkgs.tor-browser
        pkgs.tor
        pkgs.geckodriver
        pkgs.bubblewrap
      ];
      package = pkgs.rustPlatform.buildRustPackage {
        pname = "tor-mcp";
        version = "0.1.1";
        src = pkgs.lib.cleanSource self;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.makeWrapper ];
        nativeCheckInputs = [ pkgs.nodejs ] ++ runtime;
        preCheck = ''
          export TOR_MCP_BINARY=${pkgs.tor-browser}/bin/tor-browser
          export TOR_MCP_BUNDLE=${pkgs.tor-browser}/share/tor-browser
          node --test tests/page.test.cjs
        '';
        postInstall = ''
          wrapProgram $out/bin/tor-mcp \
            --prefix PATH : ${pkgs.lib.makeBinPath runtime} \
            --set TOR_MCP_BINARY ${pkgs.tor-browser}/bin/tor-browser \
            --set TOR_MCP_BUNDLE ${pkgs.tor-browser}/share/tor-browser
        '';
        meta = {
          description = "Lazy Tor Browser automation for AI agents";
          homepage = "https://github.com/willfish/tor-mcp";
          license = pkgs.lib.licenses.mit;
          platforms = [ system ];
          mainProgram = "tor-mcp";
        };
      };
    in
    {
      packages.${system}.default = package;
      apps.${system}.default = {
        type = "app";
        program = "${package}/bin/tor-mcp";
        meta.description = package.meta.description;
      };
      devShells.${system}.default = pkgs.mkShell {
        packages = [
          pkgs.cargo
          pkgs.rustc
          pkgs.rustfmt
          pkgs.clippy
          pkgs.nixfmt
          pkgs.pkg-config
          pkgs.nodejs
        ]
        ++ runtime;
        TOR_MCP_BINARY = "${pkgs.tor-browser}/bin/tor-browser";
        TOR_MCP_BUNDLE = "${pkgs.tor-browser}/share/tor-browser";
      };
      checks.${system}.default = package;
    };
}
