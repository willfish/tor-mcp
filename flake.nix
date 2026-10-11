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
        pname = "onion-browser";
        version = "0.1.0";
        src = pkgs.lib.cleanSource self;
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.makeWrapper ];
        postInstall = ''
          wrapProgram $out/bin/onion-browser \
            --prefix PATH : ${pkgs.lib.makeBinPath runtime} \
            --set ONION_BROWSER_BINARY ${pkgs.tor-browser}/bin/tor-browser \
            --set ONION_BROWSER_BUNDLE ${pkgs.tor-browser}/share/tor-browser
        '';
        meta = {
          description = "Lazy Tor Browser automation for AI agents";
          homepage = "https://github.com/willfish/onion-browser";
          license = pkgs.lib.licenses.mit;
          platforms = [ system ];
          mainProgram = "onion-browser";
        };
      };
    in
    {
      packages.${system}.default = package;
      apps.${system}.default = {
        type = "app";
        program = "${package}/bin/onion-browser";
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
        ]
        ++ runtime;
        ONION_BROWSER_BINARY = "${pkgs.tor-browser}/bin/tor-browser";
        ONION_BROWSER_BUNDLE = "${pkgs.tor-browser}/share/tor-browser";
      };
      checks.${system}.default = package;
    };
}
