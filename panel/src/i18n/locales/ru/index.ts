// Русский — исходный язык пульта: полный словарь, его тип — образец для всех языков (docs/plans/i18n.md). Новое
// пространство имён — файл здесь, строка ниже и такой же файл в каждом языке.

import common from './common';
import auth from './auth';
import settings from './settings';
import placement from './placement';
import placementMap from './placementMap';
import places from './places';
import stationsScreen from './stationsScreen';
import stationWidgets from './stationWidgets';
import helperPc from './helperPc';
import helpers from './helpers';
import devices from './devices';
import firstRun from './firstRun';
import onboarding from './onboarding';
import onbBrain from './onbBrain';
import showcase from './showcase';
import wizard from './wizard';
import deskAsk from './deskAsk';
import onbCharacter from './onbCharacter';
import onbConnect from './onbConnect';
import onbTry from './onbTry';
import onbStore from './onbStore';
import stations from './stations';
import workshop from './workshop';
import drafts from './drafts';
import fitting from './fitting';
import voiceLab from './voiceLab';
import recall from './recall';
import memory from './memory';
import sleep from './sleep';
import history from './history';
import journal from './journal';
import say from './say';
import ask from './ask';
import models from './models';
import brain from './brain';
import localAsr from './localAsr';
import voiceInput from './voiceInput';
import routes from './routes';
import mcp from './mcp';
import connect from './connect';
import connectors from './connectors';
import catalog from './catalog';
import magicplace from './magicplace';
import plugins from './plugins';
import skills from './skills';
import folders from './folders';
import news from './news';
import dashboard from './dashboard';
import desk from './desk';
import home from './home';
import house from './house';
import trip from './trip';
import characters from './characters';
import shell from './shell';
import ui from './ui';
import cards from './cards';
import bus from './bus';
import voice from './voice';
import core from './core';
import sdk from './sdk';
import frame from './frame';
import media from './media';
import share from './share';
import cloudVoices from './cloudVoices';
import router from './router';
import egress from './egress';
import cloudSpeech from './cloudSpeech';

const ru = {
    common,
    auth,
    settings,
    placement,
    placementMap,
    places,
    stationsScreen,
    stationWidgets,
    helperPc,
    helpers,
    devices,
    firstRun,
    onboarding,
    onbBrain,
    showcase,
    wizard,
    deskAsk,
    onbCharacter,
    onbConnect,
    onbTry,
    onbStore,
    stations,
    workshop,
    drafts,
    fitting,
    voiceLab,
    recall,
    memory,
    sleep,
    history,
    journal,
    say,
    ask,
    models,
    brain,
    localAsr,
    voiceInput,
    routes,
    mcp,
    connect,
    connectors,
    catalog,
    magicplace,
    plugins,
    skills,
    folders,
    news,
    dashboard,
    desk,
    home,
    house,
    trip,
    characters,
    shell,
    ui,
    cards,
    bus,
    voice,
    core,
    sdk,
    frame,
    media,
    share,
    cloudVoices,
    router,
    egress,
    cloudSpeech,
};

export type Messages = typeof ru;
export default ru;
