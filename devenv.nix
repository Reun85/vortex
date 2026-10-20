{
  pkgs,
  lib,
  inputs,
  ...
}:

let

  cmake-wrapped = pkgs.writeShellScriptBin "cmake" ''
    if [[ "$1" == "--build" || "$1" == "--install" || "$1" == "-E" ]]; then
      exec ${pkgs.cmake}/bin/cmake "$@"
    else
      exec ${pkgs.cmake}/bin/cmake -DCMAKE_POLICY_VERSION_MINIMUM=3.5 "$@"
    fi
  '';

  buildTools = with pkgs; [
    ## Real
    pkg-config
    cmake-wrapped
    clang
    cargo-watch
    gnum4
    glslang
    ninja

    vulkan-headers
    vulkan-tools
    vulkan-tools-lunarg

    ## For Dev
    cloc
    cargo-audit
    cargo-auditable
    cargo-outdated
    cargo-edit
    wgsl-analyzer
    cargo-llvm-cov
    renderdoc
#    tracy
    perf
    tombi
  ];

  # Also required at runtime
  runtimeTools = with pkgs; [

    ## Real
    vulkan-loader
    shaderc.bin
    stdenv.cc.cc.lib
    libxcursor
    libxrandr
    libxi
    wayland # Window creation system support for Wayland
    libxkbcommon # Input method support

    ## For Dev
    vulkan-validation-layers
    ## Maybe useful later

    # wayland-protocols
    # wayland-utils
    # hotspot
    # libgcc
    # dbus
    # glib
  ];

  renderdocScript = profile: settingFile: ''
    echo "Building carver_toy (${profile})..."
    cargo build --profile ${profile}

    echo "Launching RenderDoc via XWayland..."
    env -u WAYLAND_DISPLAY \
        WINIT_UNIX_BACKEND=x11 \
        WGPU_BACKEND=vulkan \
        QT_QPA_PLATFORM=xcb \
        qrenderdoc ${settingFile}
  '';
in
{
  languages.nix.enable = true;
  languages.rust = {
    enable = true;
    mold.enable = true;
    toolchainFile = ./rust-toolchain.toml;
  };
  languages.python = {
    enable = true;
    package = pkgs.python3.withPackages (
      ps: with ps; [
        pandas
        matplotlib
        seaborn
        xlsxwriter
        openpyxl
      ]
    );
  };

  packages = buildTools ++ runtimeTools;

  dotenv.enable = true;

  env = {
    VK_LAYER_PATH = "${pkgs.vulkan-validation-layers}/share/vulkan/explicit_layer.d";
    RUST_BACKTRACE = "full";
    LD_LIBRARY_PATH = "${lib.makeLibraryPath runtimeTools}";

    WAYLAND_DISPLAY = builtins.getEnv "WAYLAND_DISPLAY";
    XDG_RUNTIME_DIR = builtins.getEnv "XDG_RUNTIME_DIR";
  };

  scripts = {
    renderdoc = {
      description = "Build opt profile and launch RenderDoc";
      exec = renderdocScript "opt" "./renderdoc/opt.cap";
    };
    renderdocdev = {
      description = "Build dev profile and launch RenderDoc";
      exec = renderdocScript "dev" "./renderdoc/debug.cap";
    };
    coverage = {
      description = "Run tests and generate coverage report";
      exec = ''
        cargo llvm-cov test --doctests --all-features --branch --html --open
        echo "Coverage report generated in target/coverage/html/index.html"
      '';
    };
  };

  enterShell = ''
    data_home="''${XDG_DATA_HOME:-$HOME/.local/share}"
    data_dir="''${data_home}/reun"

    mkdir -p ''${data_dir}
    ln -sfn $(pwd)/carver_toy/state ''${data_dir}/carver

    alias renderdoc-x11="env -u WAYLAND_DISPLAY WINIT_UNIX_BACKEND=x11 WGPU_BACKEND=vulkan QT_QPA_PLATFORM=xcb qrenderdoc"
  '';
}
