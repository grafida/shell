# Fedora 43 Workstation for grafida/grafida#86: the Everything netinstall with the Workstation
# package environment (the Workstation live ISO's installer takes no kickstart). GNOME on Wayland,
# GDM. @PASSWORD_HASH@ and @SSH_KEY@ are filled in by linuxvm.sh.
text
lang en_US.UTF-8
keyboard us
timezone UTC --utc
network --bootproto=dhcp --activate --hostname=fedora43
url --mirrorlist=https://mirrors.fedoraproject.org/mirrorlist?repo=fedora-43&arch=x86_64
repo --name=updates --mirrorlist=https://mirrors.fedoraproject.org/mirrorlist?repo=updates-released-f43&arch=x86_64
rootpw --lock
user --name=tester --groups=wheel --iscrypted --password=@PASSWORD_HASH@
sshkey --username=tester "@SSH_KEY@"
zerombr
clearpart --all --initlabel
autopart
bootloader
firstboot --disable
services --enabled=sshd,gdm
poweroff

%packages
@^workstation-product-environment
openssh-server
%end

%post
systemctl set-default graphical.target
echo "tester ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/tester
printf "[daemon]\nAutomaticLoginEnable=True\nAutomaticLogin=tester\n" > /etc/gdm/custom.conf
mkdir -p /home/tester/.config && echo yes > /home/tester/.config/gnome-initial-setup-done
chown -R tester:tester /home/tester/.config
%end
