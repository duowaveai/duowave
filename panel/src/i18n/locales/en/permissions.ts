// Texts of «Permissions» (docs/plans/permissions.md): the first-steps step before the speaker search, the rows in
// Settings → Access, the guide window beside the Mac's System Settings and the drawn macOS windows.
import type { Translation } from '../../types';
import type ru from '../ru/permissions';

const en: Translation<typeof ru> = {
    stepTitleMac: 'Allow DuoWave two things',
    stepAboutMac:
        'macOS asks before a program can hear you or find devices on your home network. Press “Allow” here, then in the macOS window.',
    stepTitleWindows: 'Windows is blocking something',
    stepAboutWindows: 'Without this, DuoWave cannot find the speaker or cannot hear you. Each row has a button that fixes it.',
    later: 'Later',
    allDone: 'Everything is allowed. Moving on.',

    localNetwork: 'Local network',
    localNetworkWhy: 'To find the speaker and other computers at home.',
    microphone: 'Microphone',
    microphoneWhy: 'To hear you when you press “Speak” or the talk key.',
    microphoneWhyWindows: 'Windows has turned the microphone off for programs, so DuoWave cannot hear you.',
    firewall: 'Firewall',
    firewallWhy: 'Windows Firewall keeps the speaker from reaching this computer.',
    networkProfile: 'Public network',
    networkProfileWhy: (names: string) =>
        `Windows marks the network “${names}” as public. On such a network Windows lets nobody reach the computer.`,

    granted: 'Allowed',
    waiting: 'Waiting for your answer',
    ask: 'Allow',
    open: 'Open settings',
    allowFirewall: 'Allow in the firewall',

    noWindowMac: 'No macOS window came up, or you pressed “Don’t Allow”? Open the settings and turn DuoWave on.',
    stepsMicrophoneWindows: 'In Settings, turn on “Microphone access” and “Let desktop apps access your microphone”.',
    stepsNetworkWindows: 'Open the network this computer is on and choose “Private network”.',
    stepsFirewall: 'Windows will ask for administrator permission.',
    notHere: 'The buttons work only in the DuoWave app on this computer.',
    failed: 'That did not work. Try again.',

    pictureCaption: 'This is what the macOS window looks like. Press “Allow” in it.',
    promptLocalNetwork: 'Allow “DuoWave” to find devices on local networks?',
    promptLocalNetworkBody: 'The gateway finds the speaker on your home network and talks to it.',
    promptMicrophone: '“DuoWave” would like to access the Microphone.',
    promptMicrophoneBody: 'DuoWave hears you when you press “Speak” or the talk key, and records your voice for a voice of your own.',
    promptDeny: 'Don’t Allow',
    promptAllow: 'Allow',

    guide: 'Find DuoWave in the list and turn its switch on.',
    guideDone: 'Done. You can close the settings.',
    guideClose: 'Close the hint',

    sectionTitle: 'Permissions on this computer',
    sectionAbout: 'What the system has to allow DuoWave. The ticks update by themselves.',
    sectionEmpty: 'Nothing needs to be allowed on this computer.',

    micForComputer: 'The computer will hear you itself. Allow the microphone.',
};

export default en;
