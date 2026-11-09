#!/bin/bash
# Arch Linux with KDE Plasma (Wayland) and SDDM for grafida/grafida#86, installed by archiso's
# `script=` boot hook from the live ISO: pacstrap base + plasma-meta, what archinstall's KDE
# profile installs. @PASSWORD_HASH@ and @SSH_KEY@ are filled in by linuxvm.sh.
set -euxo pipefail
exec > >(tee /dev/ttyS0) 2>&1

until ping -c1 -W2 archlinux.org >/dev/null 2>&1; do sleep 2; done
pacman -Sy --noconfirm archlinux-keyring

sfdisk /dev/vda <<<'label: dos
,,L,*'
mkfs.ext4 -F /dev/vda1
mount /dev/vda1 /mnt
pacstrap -K /mnt base linux linux-firmware grub sudo openssh networkmanager \
    plasma-meta sddm konsole dolphin
genfstab -U /mnt >> /mnt/etc/fstab

arch-chroot /mnt /bin/bash -euxo pipefail <<'CHROOT'
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
echo 'en_US.UTF-8 UTF-8' > /etc/locale.gen && locale-gen
echo 'LANG=en_US.UTF-8' > /etc/locale.conf
echo arch > /etc/hostname
useradd -m -G wheel tester
usermod -p '@PASSWORD_HASH@' tester
echo 'tester ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/tester
install -d -m 700 -o tester -g tester /home/tester/.ssh
echo '@SSH_KEY@' > /home/tester/.ssh/authorized_keys
chown tester:tester /home/tester/.ssh/authorized_keys && chmod 600 /home/tester/.ssh/authorized_keys
mkdir -p /etc/sddm.conf.d
printf '[Autologin]\nUser=tester\nSession=plasma\n' > /etc/sddm.conf.d/autologin.conf
grub-install --target=i386-pc /dev/vda
grub-mkconfig -o /boot/grub/grub.cfg
systemctl enable sddm NetworkManager sshd
CHROOT

sync
poweroff
