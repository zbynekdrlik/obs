Name: obs-studio-plugin-loudness-dock
Version: @VERSION@
Release: @RELEASE@%{?dist}
Summary: Audio loudness dock plugin for OBS Studio
License: GPLv2+

Source0: %{name}-%{version}.tar.bz2
BuildRequires: cmake, gcc, gcc-c++
BuildRequires: obs-studio-devel
BuildRequires: qt6-qtbase-devel qt6-qtbase-private-devel
BuildRequires: libebur128-devel

%description
This is a plugin for OBS Studio to provide a dock window displaying EBU R 128 loudness meter.

%prep
%autosetup -p1
sed -i -e 's/project(obs-loudness-dock/project(loudness-dock/g' CMakeLists.txt

%build
%{cmake} -DQT_VERSION=6 -DINSTALL_LICENSE_FILES:BOOL=OFF
%{cmake_build}

%install
%{cmake_install}

%files
%{_libdir}/obs-plugins/*.so
%{_datadir}/obs/obs-plugins/*/
%license LICENSE
