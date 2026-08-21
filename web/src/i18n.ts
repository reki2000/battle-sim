/**
 * 最小限の i18n（日本語 / 英語）。
 *
 * フレームワークを使わないため、DOM は毎回作り直すのではなく、
 * 言語切替時に `onLangChange` に登録された再描画関数を呼び直す方式にする。
 */

export type Lang = "ja" | "en";

const DICT: Record<string, Record<Lang, string>> = {
  possess: { ja: "憑依", en: "Possess" },
  release: { ja: "解除", en: "Release" },
  possessing: { ja: "憑依中: #{node}", en: "Possessing: #{node}" },
  commandTree: { ja: "指揮系統", en: "Command tree" },
  orders: { ja: "命令", en: "Orders" },
  orderMove: { ja: "移動", en: "Move" },
  orderHold: { ja: "待機", en: "Hold" },
  orderAttack: { ja: "攻撃", en: "Attack" },
  orderCharge: { ja: "突撃", en: "Charge" },
  orderFlankLeft: { ja: "側面（左）", en: "Flank (L)" },
  orderFlankRight: { ja: "側面（右）", en: "Flank (R)" },
  orderWithdraw: { ja: "退却", en: "Withdraw" },
  orderShoot: { ja: "射撃", en: "Shoot" },
  orderReserve: { ja: "予備", en: "Reserve" },
  orderPursue: { ja: "追撃", en: "Pursue" },
  orderHuntPerson: { ja: "人物追跡", en: "Hunt person" },
  orderOccupyArea: { ja: "区域占領", en: "Occupy area" },
  orderGuardArea: { ja: "区域防衛", en: "Guard area" },
  orderEngineer: { ja: "築城", en: "Engineer" },
  pickPosition: { ja: "地図をクリックして目標地点を指定", en: "Click the map to set the target point" },
  pickPositionPair: { ja: "地図を 2 回クリックして区間を指定", en: "Click the map twice to set a line" },
  pickTargetNode: { ja: "指揮系統の一覧から敵部隊を選択", en: "Pick an enemy node from the list" },
  pickTargetSoldier: {
    ja: "地図上の敵兵（指揮官など）をクリック",
    en: "Click an enemy soldier on the map",
  },
  cancel: { ja: "取消", en: "Cancel" },
  deathCauses: { ja: "死因内訳", en: "Death causes" },
  kills: { ja: "撃破", en: "Kills" },
  downed: { ja: "戦闘不能", en: "Downed" },
  friendlyFire: { ja: "誤射", en: "Friendly fire" },
  soldierDetail: { ja: "兵士詳細", en: "Soldier detail" },
  commanderDetail: { ja: "指揮官詳細", en: "Commander detail" },
  hp: { ja: "体力", en: "HP" },
  morale: { ja: "士気", en: "Morale" },
  fatigue: { ja: "疲労", en: "Fatigue" },
  ammo: { ja: "残弾", en: "Ammo" },
  target: { ja: "交戦相手", en: "Target" },
  none: { ja: "なし", en: "none" },
  bravery: { ja: "勇敢さ", en: "Bravery" },
  discipline: { ja: "規律", en: "Discipline" },
  skill: { ja: "練度", en: "Skill" },
  follow: { ja: "追従", en: "Follow" },
  unfollow: { ja: "追従解除", en: "Unfollow" },
  followingSoldier: { ja: "兵士 #{id} を追従中", en: "Following soldier #{id}" },
  clickSoldierToSelect: { ja: "兵士をクリックして選択", en: "Click a soldier to select" },
  // 兵士の判断まわり（B0 のデバッグ表示）
  decisionState: { ja: "判断", en: "Decision" },
  mission: { ja: "任務", en: "Mission" },
  currentAction: { ja: "現在行動", en: "Action" },
  focusTarget: { ja: "迎撃対象", en: "Focus" },
  formationSlot: { ja: "隊形スロット", en: "Slot" },
  slotDistance: { ja: "スロットまで", en: "To slot" },
  goalPoint: { ja: "目標地点", en: "Goal" },
  reactionRadius: { ja: "反応半径", en: "Reaction radius" },
  thinkIn: { ja: "次の判断", en: "Next think" },
  formationSampleIn: { ja: "命令反応待ち", en: "Order response in" },
  commitLeft: { ja: "行動継続", en: "Commit left" },
  actionCandidates: { ja: "行動候補（点数順）", en: "Action candidates" },
  candidateFighting: { ja: "交戦中", en: "fighting" },
  candidateLoad: { ja: "向かっている味方", en: "attackers" },
  noCandidates: { ja: "候補なし（判断待ち）", en: "no candidates yet" },
  ticksUnit: { ja: "tick", en: "ticks" },
  missionNoUnit: { ja: "無所属", en: "no unit" },
  missionNoObjective: { ja: "任務なし", en: "no objective" },
  missionMoveTo: { ja: "移動", en: "Move to" },
  missionHold: { ja: "待機", en: "Hold" },
  missionAttack: { ja: "攻撃", en: "Attack" },
  missionCharge: { ja: "突撃", en: "Charge" },
  missionFlank: { ja: "側面攻撃", en: "Flank" },
  missionEnvelop: { ja: "包囲", en: "Envelop" },
  missionScreen: { ja: "遮蔽", en: "Screen" },
  missionReserve: { ja: "予備", en: "Reserve" },
  missionWithdraw: { ja: "退却", en: "Withdraw" },
  missionPursue: { ja: "追撃", en: "Pursue" },
  missionShootAt: { ja: "射撃", en: "Shoot at" },
  missionHuntPerson: { ja: "人物追跡", en: "Hunt person" },
  missionOccupyArea: { ja: "区域占領", en: "Occupy area" },
  missionGuardArea: { ja: "区域防衛", en: "Guard area" },
  actionFollowOrder: { ja: "命令に従う", en: "Follow order" },
  // 局所知覚（B1）
  perceivedNeighbours: { ja: "見えている", en: "In sight" },
  enemyCount: { ja: "敵{n}", en: "{n} enemies" },
  allyCount: { ja: "味方{n}", en: "{n} allies" },
  threatDirections: { ja: "脅威の向き", en: "Threats" },
  front: { ja: "正面", en: "front" },
  flank: { ja: "側面", en: "flank" },
  rear: { ja: "背面", en: "rear" },
  nearestEnemy: { ja: "最寄りの敵", en: "Nearest enemy" },
  supportableFight: { ja: "援護できる戦闘", en: "Supportable fight" },
  crowding: { ja: "密集", en: "Crowding" },
  localBroken: { ja: "崩れた味方", en: "Broken nearby" },
  actionJoinFight: { ja: "近くの敵へ", en: "Join fight" },
  actionSupport: { ja: "味方を援護", en: "Support" },
  actionDefend: { ja: "踏みとどまる", en: "Defend" },
  actionHoldSlot: { ja: "持ち場を維持", en: "Hold slot" },
  actionFallBack: { ja: "一歩退く", en: "Fall back" },
  actionFlee: { ja: "離脱する", en: "Flee" },
  actionRally: { ja: "再集結する", en: "Rally" },
  actionScores: { ja: "行動の点数", en: "Action scores" },
  // 任務の段階と地点（B5）
  missionState: { ja: "任務の段階", en: "Mission state" },
  missionStateIdle: { ja: "任務なし", en: "idle" },
  missionStateMoving: { ja: "移動中", en: "moving" },
  missionStateDeploying: { ja: "展開中", en: "deploying" },
  missionStateEngaged: { ja: "交戦中", en: "engaged" },
  missionStateSecured: { ja: "確保済み", en: "secured" },
  missionStateFailed: { ja: "失敗", en: "failed" },
  missionStateReturning: { ja: "帰還中", en: "returning" },
  missionArea: { ja: "任務地点", en: "Mission area" },
  pursuitLeash: { ja: "追撃限界", en: "pursuit leash" },
  insideArea: { ja: "範囲内", en: "inside" },
  perceived: { ja: "認識（指揮官視点）", en: "Perceived" },
  actual: { ja: "実際", en: "Actual" },
  forceRatio: { ja: "戦力比", en: "Force ratio" },
  momentum: { ja: "勢い", en: "Momentum" },
  decisionLog: { ja: "意思決定ログ", en: "Decision log" },
  knownEnemies: { ja: "捕捉している敵部隊", en: "Known enemy forces" },
  spectator: { ja: "観戦モード", en: "Spectator mode" },
  spectatorOn: { ja: "観戦モード: オン", en: "Spectator mode: on" },
  spectatorOff: { ja: "観戦モード: オフ", en: "Spectator mode: off" },
  saveReplay: { ja: "記録を保存", en: "Save replay" },
  loadReplay: { ja: "記録を読み込む", en: "Load replay" },
  replayMatched: { ja: "リプレイ一致（hash 一致、tick {tick}）", en: "Replay matched (hash match, tick {tick})" },
  replayMismatch: { ja: "リプレイ不一致（tick {tick}）", en: "Replay mismatch (tick {tick})" },
  battleReport: { ja: "会戦報告", en: "Battle report" },
  lang: { ja: "EN", en: "日本語" },
  battlePreset: { ja: "会戦プリセット", en: "Battle preset" },
  sandboxBattle: { ja: "対称配置（デモ）", en: "Symmetric demo" },
  sandboxNote: {
    ja: "同じ編成の 2 軍を向かい合わせに置いた練習用の会戦。",
    en: "A practice battle: two identical armies facing each other.",
  },
  showOrderOfBattle: { ja: "陣容", en: "Order of battle" },
  hideOrderOfBattle: { ja: "陣容を閉じる", en: "Hide order of battle" },
  deployed: { ja: "配置兵数", en: "Deployed" },
  battlePlan: { ja: "会戦プラン", en: "Battle plan" },
  qualityLow: { ja: "画質: 低", en: "Quality: Low" },
  qualityMedium: { ja: "画質: 中", en: "Quality: Medium" },
  qualityHigh: { ja: "画質: 高", en: "Quality: High" },
};

let lang: Lang = "ja";
const listeners: (() => void)[] = [];

export function t(key: keyof typeof DICT, vars?: Record<string, string | number>): string {
  let s = DICT[key]?.[lang] ?? key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) {
      s = s.replace(`{${k}}`, String(v)).replace(`#{${k}}`, String(v));
    }
  }
  return s;
}

export function getLang(): Lang {
  return lang;
}

export function toggleLang(): void {
  lang = lang === "ja" ? "en" : "ja";
  for (const fn of listeners) fn();
}

/** 言語切替時に再描画したい箇所を登録する。 */
export function onLangChange(fn: () => void): void {
  listeners.push(fn);
}
