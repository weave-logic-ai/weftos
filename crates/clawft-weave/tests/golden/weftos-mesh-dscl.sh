if ! dscl . -read /Groups/_weftos >/dev/null 2>&1; then
  ID=300
  while dscl . -list /Groups PrimaryGroupID | awk '{print $2}' | grep -qx "$ID" ||
        dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$ID"; do
    ID=$((ID + 1))
  done
  dscl . -create /Groups/_weftos
  dscl . -create /Groups/_weftos PrimaryGroupID "$ID"
  dscl . -create /Groups/_weftos RealName "WeftOS mesh service"
  dscl . -create /Groups/_weftos Password '*'
fi
if ! dscl . -read /Users/_weftos >/dev/null 2>&1; then
  GID_=$(dscl . -read /Groups/_weftos PrimaryGroupID | awk '{print $2}')
  UID_="$GID_"
  while dscl . -list /Users UniqueID | awk '{print $2}' | grep -qx "$UID_"; do
    UID_=$((UID_ + 1))
  done
  dscl . -create /Users/_weftos
  dscl . -create /Users/_weftos UniqueID "$UID_"
  dscl . -create /Users/_weftos PrimaryGroupID "$GID_"
  dscl . -create /Users/_weftos RealName "WeftOS mesh service"
  dscl . -create /Users/_weftos UserShell /usr/bin/false
  dscl . -create /Users/_weftos NFSHomeDirectory /var/empty
  dscl . -create /Users/_weftos Password '*'
  dscl . -create /Users/_weftos IsHidden 1
fi
