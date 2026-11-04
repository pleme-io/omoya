{ pkgs, omoya }:
let
  tool =
    name: file:
    pkgs.writers.writePython3Bin name { flakeIgnore = [ "E501" ]; } (builtins.readFile file);
in
pkgs.testers.runNixOSTest {
  name = "omoya-vkms-daily";
  skipLint = true;
  skipTypeCheck = true;
  nodes.machine =
    { ... }:
    {
      boot.kernelModules = [ "vkms" ];
      services.displayManager.enable = false;
      users.users.seat = {
        isNormalUser = true;
        password = "seat";
      };
      fonts.packages = [ pkgs.dejavu_fonts ];
      security.pam.services.swaylock = { };
      security.polkit.enable = true;
      systemd.services.systemd-suspend.serviceConfig.ExecStart = [
        ""
        "${pkgs.coreutils}/bin/true"
      ];
      environment.systemPackages = [
        omoya
        pkgs.weston
        pkgs.swaylock
        pkgs.libnotify
        pkgs.grim
        pkgs.file
        pkgs.procps
        (tool "kanshou-call" ./kanshou-call.py)
        (tool "kanshou-capture" ./kanshou-capture.py)
        (tool "ppm-share" ./ppm-share.py)
      ];
      services.getty.autologinUser = "seat";
      virtualisation.memorySize = 2048;
    };
  testScript = builtins.readFile ./vkms-daily.py;
}
